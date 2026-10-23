// SPDX-License-Identifier: MIT OR Apache-2.0
//! One catalogue and runner per application process, shared by all callers.
use crate::verbs::Refusal;
use crate::{Catalogue, Output, Roots, Runner, Status};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::SystemTime;

#[cfg(test)]
mod tests;

pub type FinishHook = Arc<dyn Fn(Value) + Send + Sync>;
pub type ChangeHook = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct Session(Arc<Mutex<State>>);

struct State {
    app: String,
    roots: Option<Roots>,
    catalogue: Option<Catalogue>,
    runner: Runner,
    run: u64,
    identity: Value,
    active: bool,
    finish: Vec<(u64, FinishHook)>,
    next_subscriber: u64,
    changes: Vec<Weak<dyn Fn() + Send + Sync>>,
    finish_delivery: Arc<Mutex<()>>,
    scan: Arc<Mutex<()>>,
}

impl Session {
    pub fn for_app(app: &str) -> Self {
        static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
        SESSIONS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .entry(app.to_owned())
            .or_insert_with(|| Self::with_roots(app, Roots::resolve()))
            .clone()
    }

    pub fn with_roots(app: &str, roots: Option<Roots>) -> Self {
        Self(Arc::new(Mutex::new(State {
            app: app.into(),
            roots,
            catalogue: None,
            runner: Runner::default(),
            run: 0,
            identity: json!({"run":null,"id":null,"title":null,"origin":null,"started":null}),
            active: false,
            finish: Vec::new(),
            next_subscriber: 0,
            changes: Vec::new(),
            finish_delivery: Arc::default(),
            scan: Arc::default(),
        })))
    }

    /// Subscribers run in registration order, outside the session lock.
    /// Remove window subscribers on close; callbacks must tolerate an in-flight delivery.
    pub fn add_finish_hook(&self, hook: FinishHook) -> u64 {
        let mut state = self.0.lock().unwrap();
        state.next_subscriber += 1;
        let id = state.next_subscriber;
        state.finish.push((id, hook));
        id
    }

    pub fn remove_finish_hook(&self, id: u64) {
        self.0.lock().unwrap().finish.retain(|(key, _)| *key != id);
    }

    pub fn replace_catalogue(&self, catalogue: Catalogue) {
        let mut state = self.0.lock().unwrap();
        let changed = state
            .catalogue
            .as_ref()
            .is_none_or(|old| old.entries != catalogue.entries);
        state.catalogue = Some(catalogue);
        drop(state);
        if changed {
            self.changed();
        }
    }

    /// The caller retains the hook; dropping it removes the observer lazily.
    pub fn observe_changes(&self, hook: &ChangeHook) {
        self.0.lock().unwrap().changes.push(Arc::downgrade(hook));
    }

    fn changed(&self) {
        let hooks = {
            let mut state = self.0.lock().unwrap();
            state.changes.retain(|hook| hook.strong_count() > 0);
            state
                .changes
                .iter()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        };
        for hook in hooks {
            hook();
        }
    }

    pub fn catalogue(&self) -> Option<Catalogue> {
        self.0.lock().unwrap().catalogue.clone()
    }

    /// Storage roots for background create/edit work in the GUI adapter.
    pub fn roots(&self) -> Option<Roots> {
        self.0.lock().unwrap().roots.clone()
    }

    pub fn runner_status(&self) -> Status {
        self.0.lock().unwrap().runner.status()
    }

    pub fn runner_output(&self) -> Output {
        self.0.lock().unwrap().runner.output()
    }

    /// Call on a worker: creation and the following scan both touch disk.
    pub fn create(&self, title: &str) -> Result<Value, Refusal> {
        let (roots, app) = {
            let state = self.0.lock().unwrap();
            (
                state
                    .roots
                    .clone()
                    .ok_or_else(|| Refusal::new("UNAVAILABLE", "an absolute HOME is required"))?,
                state.app.clone(),
            )
        };
        let path = roots.create_from_title(&app, title).map_err(|error| {
            Refusal::new(
                if error.kind() == std::io::ErrorKind::InvalidInput {
                    "INVALID_ARGUMENT"
                } else {
                    "INTERNAL"
                },
                error,
            )
        })?;
        let catalogue = self.reload()?;
        let entry = catalogue
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .ok_or_else(|| Refusal::new("INTERNAL", "created macro missing after rescan"))?;
        Ok(json!({"id":entry.id,"title":entry.title,"path":path}))
    }

    /// Call on a worker thread. No filesystem work holds the session lock.
    pub fn reload(&self) -> Result<Catalogue, Refusal> {
        let (roots, app, scan) = {
            let state = self.0.lock().unwrap();
            (
                state
                    .roots
                    .clone()
                    .ok_or_else(|| Refusal::new("UNAVAILABLE", "an absolute HOME is required"))?,
                state.app.clone(),
                state.scan.clone(),
            )
        };
        let _scan = scan.lock().unwrap();
        let catalogue = Catalogue::scan(&roots, &app).map_err(|e| Refusal::new("INTERNAL", e))?;
        self.0.lock().unwrap().catalogue = Some(catalogue.clone());
        drop(_scan);
        self.changed();
        Ok(catalogue)
    }

    /// Acceptance requires starting the manager thread. A refusal preserves the
    /// previous run number and identity and emits no finish event. Each accepted
    /// run gets exactly one finish callback, including a child spawn failure
    /// inside the manager, which happens after acceptance.
    pub fn run(&self, id: &str, origin: &str) -> Result<Value, Refusal> {
        let mut state = self.0.lock().unwrap();
        if state.active {
            return Err(Refusal::new("BUSY", "a macro is already running"));
        }
        let entry = state
            .catalogue
            .as_ref()
            .and_then(|c| c.entries.iter().find(|e| e.id == id))
            .cloned()
            .ok_or_else(|| Refusal::new("NOT_FOUND", "unknown macro id"))?;
        let roots = state
            .roots
            .clone()
            .ok_or_else(|| Refusal::new("UNAVAILABLE", "an absolute HOME is required"))?;
        let previous_run = state.run;
        let previous_identity = state.identity.clone();
        state.run += 1;
        let run = state.run;
        state.identity = json!({"run":run,"id":entry.id,"title":entry.title,"origin":origin,
            "started":SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis()});
        state.active = true;
        let weak = Arc::downgrade(&self.0);
        let delivery = state.finish_delivery.clone();
        state.runner.set_finish_hook(Some(Arc::new(move |status| {
            // A subscriber may start the next run. Its completion must wait
            // until every subscriber has received this run, without holding
            // the session's state lock during callbacks.
            let _delivery = delivery.lock().unwrap();
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let (hooks, body) = {
                let mut state = inner.lock().unwrap();
                let mut body = state.identity.clone();
                status_fields(&mut body, status);
                body["dropped"] = json!(state.runner.output().dropped);
                body["text"] = json!(state.runner.output().text);
                state.active = false;
                (state.finish.clone(), body)
            };
            for (_, hook) in hooks {
                hook(body.clone());
            }
            Session(inner).changed();
        })));
        let app = state.app.clone();
        if let Err(error) = state.runner.start(&roots, &app, &entry) {
            state.active = false;
            state.run = previous_run;
            state.identity = previous_identity;
            return Err(Refusal::new("INTERNAL", error));
        }
        drop(state);
        self.changed();
        Ok(json!({"accepted":true,"run":run,"id":entry.id,"title":entry.title}))
    }

    pub fn status(&self) -> Value {
        let state = self.0.lock().unwrap();
        let mut body = state.identity.clone();
        status_fields(&mut body, &state.runner.status());
        body
    }

    pub fn output(&self) -> Value {
        let state = self.0.lock().unwrap();
        let Output { text, dropped } = state.runner.output();
        json!({"run":state.identity["run"],"text":text,"dropped":dropped})
    }

    pub fn stop(&self) -> bool {
        let state = self.0.lock().unwrap();
        let stopping = state.active;
        if stopping {
            state.runner.stop();
        }
        stopping
    }
}

fn status_fields(body: &mut Value, status: &Status) {
    let (name, exit, error) = match status {
        Status::Idle => ("idle", None, None),
        Status::Running => ("running", None, None),
        Status::Exited(exit) => ("exited", *exit, None),
        Status::Failed(error) => ("failed", None, Some(error)),
        Status::FailedToStart(error) => ("failed_to_start", None, Some(error)),
    };
    body["state"] = json!(name);
    body["exit"] = json!(exit);
    body["error"] = json!(error);
}
