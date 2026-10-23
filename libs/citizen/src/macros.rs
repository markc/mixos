// SPDX-License-Identifier: MIT OR Apache-2.0
//! Macro transport glue shared by application Bus lanes.
use crate::outbox::Outbox;
use bus::native_client::{IncomingCommand, SupervisedClient};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
pub use ::macros::verbs::describe;

pub fn install(app: &str, service: &str, outbox: Arc<Outbox>) {
    let topic = format!("{service}.macros.finished");
    ::macros::Session::for_app(app).set_finish_hook(Some(Arc::new(move |body| {
        outbox.push(&topic, vec![body], 0);
    })));
}

/// Intercept only the named app's macro namespace. Blocking completion waits
/// run on the runtime's blocking pool; the Bus lane keeps accepting requests.
pub fn dispatch(app: &str, command: &IncomingCommand, client: Arc<SupervisedClient>) -> bool {
    let Some(suffix) = command.command.strip_prefix(&format!("{app}.macros.")) else {
        return false;
    };
    let reply = request(app, suffix, &command.body);
    let command = command.clone();
    tokio::spawn(async move {
        let answer = match reply {
            ::macros::verbs::Reply::Immediate(answer) => answer,
            deferred => tokio::task::spawn_blocking(move || deferred.wait()).await
                .unwrap_or_else(|e| Err(e.to_string())),
        };
        let (rc, body) = answer_body(answer);
        let _ = tokio::time::timeout(Duration::from_secs(2), client.respond(&command, rc, &body.to_string())).await;
    });
    true
}

pub fn request(app: &str, suffix: &str, body: &str) -> ::macros::verbs::Reply {
    match serde_json::from_str::<Value>(if body.trim().is_empty() { "{}" } else { body }) {
        Ok(body) => ::macros::verbs::handle(&::macros::Session::for_app(app), suffix, &body),
        Err(error) => ::macros::verbs::Reply::Immediate(Err(error.to_string())),
    }
}

pub fn answer_body(answer: ::macros::verbs::Answer) -> (u8, Value) {
    match answer {
        Ok(body) => (0, body),
        Err(message) => (10, json!({"error_code":"MACRO","message":message})),
    }
}
