// SPDX-License-Identifier: MIT OR Apache-2.0
//! Transport-independent macro verbs. Deferred work never scans on the caller.
use crate::{Catalogue, Session};
use serde_json::{Value, json};
use std::sync::mpsc::{self, Receiver};

pub const SUFFIXES: [&str; 7] = ["list", "reload", "run", "status", "output", "stop", "new"];
pub type Answer = Result<Value, String>;
pub enum Reply {
    Immediate(Answer),
    Deferred(Receiver<Answer>),
}

impl Reply {
    /// Only use off the UI thread, when completing a deferred reply.
    pub fn wait(self) -> Answer {
        match self {
            Self::Immediate(answer) => answer,
            Self::Deferred(rx) => rx.recv().map_err(|e| e.to_string())?,
        }
    }
}

pub fn describe(app: &str) -> Vec<Value> {
    SUFFIXES.into_iter().map(|suffix| {
        let (description, args) = match suffix {
            "list" => ("List the last macro catalogue, scanning first if needed", json!({})),
            "reload" => ("Rescan the macro catalogue", json!({})),
            "run" => ("Start a macro by id; answers acceptance, not completion", json!({"id":"string"})),
            "status" => ("Read the current or last macro run status", json!({})),
            "output" => ("Read the retained 64 KiB output tail", json!({})),
            "stop" => ("Request termination of the current macro process group", json!({})),
            _ => ("Create a user macro (not yet available)", json!({"title":"string"})),
        };
        json!({"name":format!("{app}.macros.{suffix}"),"args":args,
            "description":description,"read_only":matches!(suffix,"list"|"status"|"output")})
    }).collect()
}

pub fn handle(session: &Session, suffix: &str, body: &Value) -> Reply {
    let valid = body.as_object().is_some_and(|object| {
        let key = match suffix { "run" => Some("id"), "new" => Some("title"), _ => None };
        object.keys().all(|k| Some(k.as_str()) == key)
            && key.is_none_or(|k| body[k].as_str().is_some_and(|s| !s.is_empty()))
    });
    if !valid { return Reply::Immediate(Err("invalid macro arguments".into())); }
    match suffix {
        "list" if session.catalogue().is_some() => Reply::Immediate(Ok(list(session.catalogue().unwrap()))),
        "list" | "reload" => defer(session, |s| s.reload().map(list)),
        "run" if session.catalogue().is_none() => {
            let id = body["id"].as_str().unwrap().to_owned();
            defer(session, move |s| { s.reload()?; s.run(&id, "bus") })
        }
        "run" => Reply::Immediate(session.run(body["id"].as_str().unwrap(), "bus")),
        "status" => Reply::Immediate(Ok(session.status())),
        "output" => Reply::Immediate(Ok(session.output())),
        "stop" => Reply::Immediate(Ok(json!({"stopping":session.stop()}))),
        "new" => Reply::Immediate(Err("not yet available".into())),
        _ => Reply::Immediate(Err("unknown macro verb".into())),
    }
}

fn defer(session: &Session, work: impl FnOnce(Session) -> Answer + Send + 'static) -> Reply {
    let session = session.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(work(session)); });
    Reply::Deferred(rx)
}

fn list(catalogue: Catalogue) -> Value {
    json!({"scanned":true,"macros":catalogue.entries.iter().map(|e| json!({
        "id":e.id,"title":e.title,"tier":e.tier.as_str(),"path":e.path,
        "shadows":e.shadows.iter().map(|t| t.as_str()).collect::<Vec<_>>()
    })).collect::<Vec<_>>(),"diagnostics":catalogue.diagnostics.iter().map(|d|
        json!({"path":d.path,"message":d.message})).collect::<Vec<_>>()})
}
