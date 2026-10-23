// SPDX-License-Identifier: MIT OR Apache-2.0
//! Macro transport glue shared by application Bus lanes.
use crate::outbox::Outbox;
pub use ::macros::verbs::{Refusal, SUFFIXES, describe};
use bus::native_client::{IncomingCommand, SupervisedClient};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

// Match Term's request envelope limit without coupling citizen to term-core.
const REQUEST_BODY_LIMIT: usize = 8192;

pub fn install(app: &str, service: &str, outbox: Arc<Outbox>) {
    static PUBLISHERS: OnceLock<Mutex<HashMap<(String, String), u64>>> = OnceLock::new();
    let mut publishers = PUBLISHERS.get_or_init(Default::default).lock().unwrap();
    let session = ::macros::Session::for_app(app);
    let key = (app.to_owned(), service.to_owned());
    if let Some(previous) = publishers.remove(&key) {
        session.remove_finish_hook(previous);
    }
    let topic = format!("{service}.macros.finished");
    let subscriber = session.add_finish_hook(Arc::new(move |body| {
        outbox.push(&topic, vec![body], 0);
    }));
    publishers.insert(key, subscriber);
}

/// Intercept only the named app's macro namespace. Blocking completion waits
/// run on the runtime's blocking pool; the Bus lane keeps accepting requests.
pub fn dispatch(app: &str, command: &IncomingCommand, client: Arc<SupervisedClient>) -> bool {
    let Some(suffix) = command.command.strip_prefix(&format!("{app}.macros.")) else {
        return false;
    };
    let reply = request(app, suffix, &command.body);
    let command = IncomingCommand {
        generation: command.generation,
        from: command.from.clone(),
        command: command.command.clone(),
        id: command.id.clone(),
        args: command.args.clone(),
        body: command.body.clone(),
        headers: command.headers.clone(),
    };
    tokio::spawn(async move {
        let answer = match reply {
            ::macros::verbs::Reply::Immediate(answer) => answer,
            deferred => tokio::task::spawn_blocking(move || deferred.wait())
                .await
                .unwrap_or_else(|e| Err(Refusal::new("INTERNAL", e))),
        };
        let (rc, body) = answer_body(answer);
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            client.respond(&command, rc, &body.to_string()),
        )
        .await;
    });
    true
}

pub fn request(app: &str, suffix: &str, body: &str) -> ::macros::verbs::Reply {
    if body.len() > REQUEST_BODY_LIMIT {
        return ::macros::verbs::Reply::Immediate(Err(Refusal::new(
            "INVALID_ARGUMENT",
            "request exceeds 8192 bytes",
        )));
    }
    match serde_json::from_str::<Value>(if body.trim().is_empty() { "{}" } else { body }) {
        Ok(body) => ::macros::verbs::handle(&::macros::Session::for_app(app), suffix, &body),
        Err(error) => {
            ::macros::verbs::Reply::Immediate(Err(Refusal::new("INVALID_ARGUMENT", error)))
        }
    }
}

pub fn answer_body(answer: ::macros::verbs::Answer) -> (u8, Value) {
    match answer {
        Ok(body) => (0, body),
        Err(refusal) => (
            10,
            json!({"error_code":refusal.code,"message":refusal.message}),
        ),
    }
}
