// SPDX-License-Identifier: MIT OR Apache-2.0
//! One catalogue and runner per application process, shared by all callers.
use crate::{Catalogue, Output, Roots, Runner, Status};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

#[cfg(test)]
mod tests;

pub type FinishHook = Arc<dyn Fn(Value) + Send + Sync>;

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
    finish: Option<FinishHook>,
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
            finish: None,
        })))
    }

    pub fn set_finish_hook(&self, hook: Option<FinishHook>) {
        self.0.lock().unwrap().finish = hook;
    }

    pub fn catalogue(&self) -> Option<Catalogue> {
        self.0.lock().unwrap().catalogue.clone()
    }

    /// Call on a worker thread. No filesystem work holds the session lock.
    pub fn reload(&self) -> Result<Catalogue, String> {
        let (roots, app) = {
            let state = self.0.lock().unwrap();
            (
                state.roots.clone().ok_or("an absolute HOME is required")?,
                state.app.clone(),
            )
        };
        let catalogue = Catalogue::scan(&roots, &app).map_err(|e| e.to_string())?;
        self.0.lock().unwrap().catalogue = Some(catalogue.clone());
        Ok(catalogue)
    }

    pub fn run(&self, id: &str, origin: &str) -> Result<Value, String> {
        let mut state = self.0.lock().unwrap();
        if state.active {
            return Err("a macro is already running".into());
        }
        let entry = state
            .catalogue
            .as_ref()
            .and_then(|c| c.entries.iter().find(|e| e.id == id))
            .cloned()
            .ok_or("unknown macro id")?;
        let roots = state.roots.clone().ok_or("an absolute HOME is required")?;
        state.run += 1;
        let run = state.run;
        state.identity = json!({"run":run,"id":entry.id,"title":entry.title,"origin":origin,
            "started":SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis()});
        state.active = true;
        let weak = Arc::downgrade(&self.0);
        state.runner.set_finish_hook(Some(Arc::new(move |status| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let (hook, body) = {
                let mut state = inner.lock().unwrap();
                let mut body = state.identity.clone();
                status_fields(&mut body, status);
                body["dropped"] = json!(state.runner.output().dropped);
                state.active = false;
                (state.finish.clone(), body)
            };
            if let Some(hook) = hook {
                hook(body);
            }
        })));
        let app = state.app.clone();
        if let Err(error) = state.runner.start(&roots, &app, &entry) {
            state.active = false;
            let mut body = state.identity.clone();
            status_fields(&mut body, &state.runner.status());
            body["dropped"] = json!(false);
            let hook = state.finish.clone();
            drop(state);
            if let Some(hook) = hook {
                hook(body);
            }
            return Err(error.to_string());
        }
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
