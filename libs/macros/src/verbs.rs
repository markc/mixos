// SPDX-License-Identifier: MIT OR Apache-2.0
//! Transport-independent macro verbs. Deferred work never scans on the caller.
use crate::{Catalogue, Session};
use serde_json::{Value, json};
use std::sync::mpsc::{self, Receiver};

pub const SUFFIXES: [&str; 11] = ["list", "reload", "run", "status", "output", "stop", "new", "get", "put", "remove", "move"];
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

impl Refusal {
    pub fn new(code: &'static str, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refusal {}

pub type Answer = Result<Value, Refusal>;
pub enum Reply {
    Immediate(Answer),
    Deferred(Receiver<Answer>),
}

impl Reply {
    /// Only use off the UI thread, when completing a deferred reply.
    pub fn wait(self) -> Answer {
        match self {
            Self::Immediate(answer) => answer,
            Self::Deferred(rx) => rx.recv().map_err(|e| Refusal::new("INTERNAL", e))?,
        }
    }
}

pub fn describe(app: &str) -> Vec<Value> {
    SUFFIXES
        .into_iter()
        .map(|suffix| {
            let (description, args) = match suffix {
                "list" => (
                    "List the last macro catalogue, scanning first if needed",
                    json!({}),
                ),
                "reload" => ("Rescan the macro catalogue", json!({})),
                "run" => (
                    "Start a macro by id; answers acceptance, not completion",
                    json!({"id":"string"}),
                ),
                "status" => ("Read the current or last macro run status", json!({})),
                "output" => ("Read the retained 64 KiB output tail", json!({})),
                "stop" => (
                    "Request termination of the current macro process group",
                    json!({}),
                ),
                "get" => ("Read macro text (at most 256 KiB)", json!({"id":"string"})),
                "put" => ("Save a user macro; request cap 272 KiB, text cap 256 KiB", json!({"id":"string","text":"string"})),
                "remove" => ("Delete a user macro", json!({"id":"string"})),
                "move" => ("Move a user macro between user tiers", json!({"id":"string","tier":"user-app or user-global"})),
                _ => (
                    "Create a user per-app macro without opening an editor",
                    json!({"title":"string"}),
                ),
            };
            json!({"name":format!("{app}.macros.{suffix}"),"args":args,
            "description":description,"read_only":matches!(suffix,"list"|"status"|"output"|"get")})
        })
        .collect()
}

pub fn handle(session: &Session, suffix: &str, body: &Value) -> Reply {
    let valid = body.as_object().is_some_and(|object| {
        let (required, allowed): (&[&str], &[&str]) = match suffix {
            "run" => (&["id"], &["id", "target"]),
            "get" | "remove" => (&["id"], &["id"]),
            "put" => (&["id", "text"], &["id", "text"]),
            "move" => (&["id", "tier"], &["id", "tier", "destination"]),
            "new" => (&["title"], &["title"]),
            _ => (&[], &[]),
        };
        object.keys().all(|k| allowed.contains(&k.as_str()) && body[k].is_string())
            && required.iter().all(|k| body[*k].as_str().is_some_and(|s| *k == "text" || !s.is_empty()))
    });
    if !valid {
        return Reply::Immediate(Err(Refusal::new(
            "INVALID_ARGUMENT",
            "invalid macro arguments",
        )));
    }
    match suffix {
        "list" if session.catalogue().is_some() => {
            Reply::Immediate(Ok(list(session.catalogue().unwrap())))
        }
        "list" | "reload" => defer(session, |s| s.reload().map(list)),
        "get" | "put" | "remove" | "move" => {
            let operation = suffix.to_owned();
            let body = body.clone();
            defer(session, move |s| s.manage(&operation, &body))
        }
        "run" if session.catalogue().is_none() => {
            let id = body["id"].as_str().unwrap().to_owned();
            let target = body["target"].as_str().map(str::to_owned);
            defer(session, move |s| {
                s.reload()?;
                s.run_with(&id, "bus", target.as_deref())
            })
        }
        "run" => Reply::Immediate(session.run_with(body["id"].as_str().unwrap(), "bus", body["target"].as_str())),
        "status" => Reply::Immediate(Ok(session.status())),
        "output" => Reply::Immediate(Ok(session.output())),
        "stop" => Reply::Immediate(Ok(json!({"stopping":session.stop()}))),
        "new" => {
            let title = body["title"].as_str().unwrap().to_owned();
            defer(session, move |s| s.create(&title))
        }
        _ => Reply::Immediate(Err(Refusal::new("INVALID_ARGUMENT", "unknown macro verb"))),
    }
}

fn defer(session: &Session, work: impl FnOnce(Session) -> Answer + Send + 'static) -> Reply {
    let session = session.clone();
    let (tx, rx) = mpsc::channel();
    match std::thread::Builder::new()
        .name("macro-scan".into())
        .spawn(move || {
            let _ = tx.send(work(session));
        }) {
        Ok(_) => Reply::Deferred(rx),
        Err(error) => Reply::Immediate(Err(Refusal::new("INTERNAL", error))),
    }
}

fn list(catalogue: Catalogue) -> Value {
    json!({"scanned":true,"macros":catalogue.entries.iter().map(|e| json!({
        "id":e.id,"title":e.title,"tier":e.tier.as_str(),"path":e.path,
        "shadows":e.shadows.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
        "targets":match &e.targets { None => Value::Null, Some(crate::Targets::Any) => json!("any"), Some(crate::Targets::List(apps)) => json!(apps) }
    })).collect::<Vec<_>>(),"diagnostics":catalogue.diagnostics.iter().map(|d|
        json!({"path":d.path,"message":d.message})).collect::<Vec<_>>()})
}
