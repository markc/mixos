// SPDX-License-Identifier: MIT OR Apache-2.0
//! Macro transport glue shared by application Bus lanes.
use crate::outbox::Outbox;
pub use ::macros::commands::{Refusal, SUFFIXES, describe};
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
#[derive(Default)]
pub struct Jobs(tokio::task::JoinSet<()>);

impl Jobs {
    /// Drain accepted macro replies before closing the native connection.
    pub async fn finish(&mut self) {
        if tokio::time::timeout(Duration::from_secs(2), async {
            while self.0.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            self.0.abort_all();
        }
    }
}

pub fn dispatch(
    app: &str,
    command: &IncomingCommand,
    client: Arc<SupervisedClient>,
    jobs: &mut Jobs,
) -> bool {
    let Some(suffix) = command.command.strip_prefix(&format!("{app}.macros.")) else {
        return false;
    };
    // Workshop owns app-targeted macro commands and opening the editor.
    if app == "workshop"
        && command.body.len()
            <= if suffix == "put" {
                ::macros::commands::PUT_ENVELOPE_LIMIT
            } else {
                REQUEST_BODY_LIMIT
            }
        && (matches!(suffix, "new" | "open")
            || serde_json::from_str::<Value>(&command.body)
                .ok()
                .is_some_and(|body| body.get("app").is_some()))
    {
        return false;
    }
    while jobs.0.try_join_next().is_some() {}
    let reply = if jobs.0.len() >= 32 {
        ::macros::commands::Reply::Immediate(Err(Refusal::new(
            "BUSY",
            "macro request capacity exhausted",
        )))
    } else {
        request(app, suffix, &command.body)
    };
    let command = IncomingCommand {
        generation: command.generation,
        from: command.from.clone(),
        command: command.command.clone(),
        id: command.id.clone(),
        args: command.args.clone(),
        body: command.body.clone(),
        headers: command.headers.clone(),
    };
    jobs.0.spawn(async move {
        let answer = match reply {
            ::macros::commands::Reply::Immediate(answer) => answer,
            deferred => tokio::task::spawn_blocking(move || deferred.wait())
                .await
                .unwrap_or_else(|e| Err(Refusal::new("INTERNAL", e))),
        };
        let (rc, body) = answer_body(answer);
        crate::debug::answered(
            command.command.split('.').next().unwrap_or("app"),
            &command.command,
            rc,
        );
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            client.respond(&command, rc, &body.to_string()),
        )
        .await;
    });
    true
}

pub fn request(app: &str, suffix: &str, body: &str) -> ::macros::commands::Reply {
    let limit = if suffix == "put" {
        ::macros::commands::PUT_ENVELOPE_LIMIT
    } else {
        REQUEST_BODY_LIMIT
    };
    if body.len() > limit {
        return ::macros::commands::Reply::Immediate(Err(Refusal::new(
            "INVALID_ARGUMENT",
            format!("request exceeds {limit} bytes"),
        )));
    }
    match serde_json::from_str::<Value>(if body.trim().is_empty() { "{}" } else { body }) {
        Ok(body) => ::macros::commands::handle(&::macros::Session::for_app(app), suffix, &body),
        Err(error) => {
            ::macros::commands::Reply::Immediate(Err(Refusal::new("INVALID_ARGUMENT", error)))
        }
    }
}

pub fn answer_body(answer: ::macros::commands::Answer) -> (u8, Value) {
    match answer {
        Ok(body) => (0, body),
        Err(refusal) => (
            10,
            json!({"error_code":refusal.code,"message":refusal.message}),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finish_abandons_a_stalled_reply() {
        let mut jobs = Jobs::default();
        jobs.0.spawn(std::future::pending());
        tokio::time::timeout(Duration::from_secs(3), jobs.finish())
            .await
            .unwrap();
        while jobs.0.join_next().await.is_some() {}
    }

    #[test]
    fn escape_heavy_put_reaches_decoded_text_validation() {
        let text = "\r\n\"".repeat(70_000);
        assert!(text.len() < 256 * 1024);
        let body = json!({"id":"missing", "text":text}).to_string();
        assert!(body.len() > 272 * 1024);
        // No existing id is required to prove that the envelope was admitted.
        let error = request("escape-heavy-test", "put", &body)
            .wait()
            .unwrap_err();
        assert_ne!(error.code, "INVALID_ARGUMENT");
    }
}
