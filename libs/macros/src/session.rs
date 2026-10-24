// SPDX-License-Identifier: MIT OR Apache-2.0
//! One catalogue and runner per application process, shared by all callers.
use crate::verbs::Refusal;
use crate::{Catalogue, Output, Roots, Runner, Status, Tier};
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
    closing: bool,
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
            closing: false,
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

    /// Management runs on a worker, using the same confined storage as the editor.
    pub fn manage(&self, operation: &str, body: &Value) -> Result<Value, Refusal> {
        let catalogue = self.reload()?;
        let entry = catalogue.entries.iter().find(|entry| Some(entry.id.as_str()) == body["id"].as_str())
            .ok_or_else(|| Refusal::new("NOT_FOUND", "unknown macro id"))?;
        let roots = self.roots().ok_or_else(|| Refusal::new("UNAVAILABLE", "macro roots unavailable"))?;
        let storage_error = |error: std::io::Error| Refusal::new(match error.kind() {
            std::io::ErrorKind::AlreadyExists => "ALREADY_EXISTS",
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::PermissionDenied => "DENIED",
            std::io::ErrorKind::NotFound => "NOT_FOUND",
            _ => "INTERNAL",
        }, error);
        if operation == "get" {
            let text = roots.read(entry).map_err(storage_error)?;
            if text.len() > 256 * 1024 {
                return Err(Refusal::new("RESOURCE_EXHAUSTED", "macro exceeds 256 KiB"));
            }
            return Ok(json!({"id":entry.id,"tier":entry.tier.as_str(),"path":entry.path,"text":text}));
        }
        if !entry.tier.is_user() {
            return Err(Refusal::new("DENIED", "built-in macros are read-only"));
        }
        let path = match operation {
            "put" => {
                let text = body["text"].as_str().ok_or_else(|| Refusal::new("INVALID_ARGUMENT", "text is required"))?;
                if text.len() > 256 * 1024 {
                    return Err(Refusal::new("INVALID_ARGUMENT", "macro exceeds 256 KiB"));
                }
                roots.write(entry, text).map_err(storage_error)?;
                entry.path.clone()
            }
            "remove" => {
                roots.remove(entry).map_err(storage_error)?;
                entry.path.clone()
            }
            "move" => {
                let tier = match body["tier"].as_str() {
                    Some("user-global") => Tier::UserGlobal,
                    Some("user-app") => Tier::UserApp,
                    _ => return Err(Refusal::new("INVALID_ARGUMENT", "expected user-global or user-app")),
                };
                let owner = self.0.lock().unwrap().app.clone();
                let app = body["destination"].as_str().unwrap_or(&owner);
                if tier == Tier::UserApp && app == "workshop" {
                    return Err(Refusal::new("INVALID_ARGUMENT", "choose a destination app"));
                }
                roots.move_to(entry, tier, app).map_err(storage_error)?
            }
            _ => return Err(Refusal::new("INVALID_ARGUMENT", "unknown management operation")),
        };
        self.reload()?;
        Ok(json!({"id":entry.id,"path":path,"changed":true}))
    }

    pub fn runner_status(&self) -> Status {
        self.0.lock().unwrap().runner.status()
    }

    pub fn runner_output(&self) -> Output {
        self.0.lock().unwrap().runner.output()
    }

    /// Call on a worker: creation and the following scan both touch disk.
    pub fn create(&self, title: &str) -> Result<Value, Refusal> {
        self.create_with_scan(title, || self.reload())
    }

    fn create_with_scan(
        &self,
        title: &str,
        scan: impl FnOnce() -> Result<Catalogue, Refusal>,
    ) -> Result<Value, Refusal> {
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
        let tier = if app == "workshop" {
            Tier::UserGlobal
        } else {
            Tier::UserApp
        };
        let path = roots
            .create_from_title_in(&app, tier, title)
            .map_err(|error| {
                Refusal::new(
                    if error.kind() == std::io::ErrorKind::InvalidInput {
                        "INVALID_ARGUMENT"
                    } else {
                        "INTERNAL"
                    },
                    error,
                )
            })?;
        let id = path.file_stem().unwrap().to_string_lossy();
        let mut answer = json!({"id":id,"title":title,"path":path,"scanned":false});
        match scan() {
            Ok(catalogue) => {
                if let Some(entry) = catalogue.entries.iter().find(|entry| entry.path == path) {
                    answer["title"] = json!(entry.title);
                    answer["scanned"] = json!(true);
                } else {
                    answer["warning"] = json!("created macro missing after rescan");
                }
            }
            Err(error) => answer["warning"] = json!(error.to_string()),
        }
        Ok(answer)
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
        let catalogue = if app == "workshop" {
            Catalogue::scan_globals(&roots)
        } else {
            Catalogue::scan(&roots, &app)
        }
        .map_err(|e| Refusal::new("INTERNAL", e))?;
        drop(_scan);
        self.replace_catalogue(catalogue.clone());
        Ok(catalogue)
    }

    /// Acceptance requires starting the manager thread. A refusal preserves the
    /// previous run number and identity and emits no finish event. Each accepted
    /// run gets exactly one finish callback, including a child spawn failure
    /// inside the manager, which happens after acceptance.
    pub fn run(&self, id: &str, origin: &str) -> Result<Value, Refusal> {
        self.run_with(id, origin, None)
    }

    pub fn run_with(&self, id: &str, origin: &str, target: Option<&str>) -> Result<Value, Refusal> {
        let mut state = self.0.lock().unwrap();
        if state.closing { return Err(Refusal::new("UNAVAILABLE", "macro session is closing")); }
        if state.active {
            return Err(Refusal::new("BUSY", "a macro is already running"));
        }
        let entry = state
            .catalogue
            .as_ref()
            .and_then(|c| c.entries.iter().find(|e| e.id == id))
            .cloned()
            .ok_or_else(|| Refusal::new("NOT_FOUND", "unknown macro id"))?;
        if target.is_some_and(|target| {
            entry
                .targets
                .as_ref()
                .is_none_or(|allowed| !allowed.allows(target))
        }) {
            return Err(Refusal::new(
                "INVALID_ARGUMENT",
                "macro target is not allowed",
            ));
        }
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
        if let Err(error) = state.runner.start_with(&roots, &app, &entry, target) {
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

    /// Call on a worker before closing the Bus. Fence new runs, terminate the
    /// process group and wait for the runner's completion delivery.
    pub fn shutdown(&self) -> Result<(), Refusal> {
        let (tx, rx) = std::sync::mpsc::channel();
        let hook = self.add_finish_hook(Arc::new(move |_| { let _ = tx.send(()); }));
        let active = {
            let mut state = self.0.lock().unwrap();
            state.closing = true;
            state.runner.stop();
            state.active
        };
        let result = if active {
            rx.recv_timeout(std::time::Duration::from_secs(15)).map_err(|e| Refusal::new("UNAVAILABLE", e))
        } else { Ok(()) };
        self.remove_finish_hook(hook);
        result
    }

    /// Check ownership and request termination under the same session lock.
    pub fn stop_if_origin(&self, origin: &str) -> bool {
        let state = self.0.lock().unwrap();
        let stopping = state.active && state.identity["origin"].as_str() == Some(origin);
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
