// SPDX-License-Identifier: MIT OR Apache-2.0
//! File and application effects, called only from ced's Bus worker.
use std::path::Path;
use std::time::Duration;

pub fn save(path: &str, text: &str, expected: Option<&str>) -> Result<(), String> {
    // Validate before touching the destination. Use the native editor's
    // replacing writer, retaining permissions and following the target link.
    maildoc::parse(text).map_err(|e| e.to_string())?;
    if let Some(expected) = expected {
        let current = match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        };
        if current.is_some_and(|text| text != expected) {
            return Err(
                "the mail document changed on disk; save to another file or reopen it".into(),
            );
        }
    }
    let mut document = editor::local::Document::open_bytes(Path::new(path), text.as_bytes())
        .map_err(|e| e.to_string())?;
    document.save_as(path).map_err(|e| e.to_string())
}

pub async fn send(client: &impl citizen::app::AppClient, path: &str) -> Result<String, String> {
    match citizen::call_app_with_limit(
        client,
        "dev.mixos.email",
        "email.send",
        serde_json::json!({"document":path}),
        Duration::from_secs(10),
        Duration::from_secs(120),
    )
    .await
    {
        Ok(reply) => {
            let value: serde_json::Value = serde_json::from_str(&reply.body).unwrap_or_default();
            Ok(value["message"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("Sent")
                .to_owned())
        }
        Err(citizen::app::AppCallError::Refused { message, .. }) => Err(message),
        Err(citizen::app::AppCallError::Transport(error)) if error.outcome_unknown => {
            Err(format!("the mail may have been sent; {}", error.message))
        }
        Err(citizen::app::AppCallError::Transport(error)) => {
            Err(format!("could not contact Email: {}", error.message))
        }
        Err(citizen::app::AppCallError::LaunchFailed(message)) => {
            Err(format!("could not launch Email: {message}"))
        }
        Err(citizen::app::AppCallError::DidNotAppear) => {
            Err("Email did not appear before the wait limit".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citizen::{CallError, Handle, Logged, Reply};

    #[tokio::test]
    async fn email_refusal_is_verbatim_and_has_only_the_document_argument() {
        let message = "Choose an identity belonging to this account.";
        let client = Handle::scripted(vec![Ok(Reply {
            rc: 10,
            body: serde_json::json!({"message":message}).to_string(),
            error: None,
        })]);
        assert_eq!(
            send(&client, "/home/user/letter.md").await.unwrap_err(),
            message
        );
        let log = client.log();
        assert_eq!(log.len(), 1);
        assert!(
            matches!(&log[0], Logged::Call { service, command, body } if service == "email" && command == "email.send" && serde_json::from_str::<serde_json::Value>(body).unwrap() == serde_json::json!({"document":"/home/user/letter.md"}))
        );
    }

    #[tokio::test]
    async fn uncertain_send_is_never_retried() {
        let client = Handle::scripted(vec![Err(CallError {
            message: "lost reply".into(),
            outcome_unknown: true,
        })]);
        let error = send(&client, "/home/user/letter.md").await.unwrap_err();
        assert!(error.contains("may have been sent"));
        assert_eq!(client.log().len(), 1);
    }

    #[tokio::test]
    async fn send_has_a_full_two_minute_deadline_and_timeout_is_uncertain() {
        struct Deadline;
        impl citizen::app::AppClient for Deadline {
            async fn app_call<'a>(&'a self, _service: &'a str, command: &'a str, _body: &'a serde_json::Value, limit: Duration) -> Result<Reply, CallError> {
                assert_eq!(command, "email.send");
                assert_eq!(limit, Duration::from_secs(120));
                Err(CallError::from("Bus request timed out"))
            }
        }
        let message = send(&Deadline, "/home/user/letter.md").await.unwrap_err();
        assert!(message.contains("may have been sent"));
        assert!(!message.contains("could not contact Email"));
    }

    #[tokio::test]
    async fn registration_wait_does_not_shorten_a_delivered_send() {
        struct Starting(std::sync::atomic::AtomicUsize);
        impl citizen::app::AppClient for Starting {
            async fn app_call<'a>(&'a self, _service: &'a str, command: &'a str, _body: &'a serde_json::Value, limit: Duration) -> Result<Reply, CallError> {
                let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                match n {
                    0 => Ok(Reply { rc:10, error:Some("Service 'email' not found".into()), ..Reply::default() }),
                    1 => { assert_eq!(command, "apps.launch"); Ok(Reply::default()) }
                    2 => {
                        assert_eq!(command, "email.send");
                        assert_eq!(limit, Duration::from_secs(120));
                        Err(CallError::from("Bus request timed out"))
                    }
                    _ => panic!("an uncertain send must not be retried"),
                }
            }
        }
        let client = Starting(std::sync::atomic::AtomicUsize::new(0));
        assert!(send(&client, "/home/user/letter.md").await.unwrap_err().contains("may have been sent"));
        assert_eq!(client.0.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[test]
    fn saves_round_trip_and_validation_or_disk_changes_leave_the_file_alone() {
        let directory = std::env::temp_dir().join(format!("ced-mail-save-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("letter.md");
        let path = path.to_str().unwrap();
        let text = "---\nmail-to: bob@example.org\n---\nBody";
        std::fs::write(path, text).unwrap();
        save(path, text, Some(text)).unwrap();
        assert_eq!(
            maildoc::parse(&std::fs::read_to_string(path).unwrap()).unwrap(),
            maildoc::parse(text).unwrap()
        );
        assert!(save(path, "---\nmail-to: invalid\n---\nBody", Some(text)).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), text);
        std::fs::write(path, "changed by another editor").unwrap();
        assert!(save(path, text, Some(text)).is_err());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "changed by another editor"
        );
        std::fs::remove_file(path).unwrap();
        save(path, text, Some(text)).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), text);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
