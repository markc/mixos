// SPDX-License-Identifier: MIT OR Apache-2.0
//! File and application effects, called only from ced's Bus worker.
use std::path::Path;
use std::time::Duration;

pub fn save(path: &str, text: &str, expected: Option<&str>) -> Result<(), String> {
    // Validate before touching the destination. Use the native editor's
    // replacing writer, retaining permissions and following the target link.
    maildoc::parse(text).map_err(|e| e.to_string())?;
    if let Some(expected) = expected {
        let current = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        if current != expected { return Err("the mail document changed on disk; save to another file or reopen it".into()); }
    }
    let mut document = editor::local::Document::open_bytes(Path::new(path), text.as_bytes()).map_err(|e| e.to_string())?;
    document.save_as(path).map_err(|e| e.to_string())
}

pub async fn send(client: &impl citizen::app::AppClient, path: &str) -> Result<String, String> {
    match citizen::call_app(client, "dev.mixos.email", "email.send", serde_json::json!({"document":path}), Duration::from_secs(10)).await {
        Ok(reply) => {
            let value: serde_json::Value = serde_json::from_str(&reply.body).unwrap_or_default();
            Ok(value["message"].as_str().filter(|s| !s.is_empty()).unwrap_or("Sent").to_owned())
        }
        Err(citizen::app::AppCallError::Refused { message, .. }) => Err(message),
        Err(citizen::app::AppCallError::Transport(error)) if error.outcome_unknown => Err(format!("the mail may have been sent; {}", error.message)),
        Err(citizen::app::AppCallError::Transport(error)) => Err(format!("could not contact Email: {}", error.message)),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citizen::{Handle, Reply, CallError, Logged};

    #[tokio::test]
    async fn email_refusal_is_verbatim_and_has_only_the_document_argument() {
        let message = "Choose an identity belonging to this account.";
        let client = Handle::scripted(vec![Ok(Reply { rc: 10, body: serde_json::json!({"message":message}).to_string(), error: None })]);
        assert_eq!(send(&client, "/home/user/letter.md").await.unwrap_err(), message);
        let log = client.log();
        assert_eq!(log.len(), 1);
        assert!(matches!(&log[0], Logged::Call { service, command, body } if service == "email" && command == "email.send" && serde_json::from_str::<serde_json::Value>(body).unwrap() == serde_json::json!({"document":"/home/user/letter.md"})));
    }

    #[tokio::test]
    async fn uncertain_send_is_never_retried() {
        let client = Handle::scripted(vec![Err(CallError { message: "lost reply".into(), outcome_unknown: true })]);
        let error = send(&client, "/home/user/letter.md").await.unwrap_err();
        assert!(error.contains("may have been sent"));
        assert_eq!(client.log().len(), 1);
    }
}
