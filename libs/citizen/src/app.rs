// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Call an app over its existing connection, launching only when the broker
//! says the service is absent. A refusal or uncertain command outcome is
//! never retried: the command may have effects.

use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use bus::native_client::SupervisedClient;
use serde_json::{Value, json};

use crate::{CallError, Handle, Reply};

/// The two native clients used by application effect workers.
pub trait AppClient: Sync {
    fn app_call<'a>(
        &'a self,
        service: &'a str,
        command: &'a str,
        body: &'a Value,
        limit: Duration,
    ) -> impl Future<Output = Result<Reply, CallError>> + Send + 'a;
}

impl AppClient for Handle {
    async fn app_call<'a>(
        &'a self,
        service: &'a str,
        command: &'a str,
        body: &'a Value,
        limit: Duration,
    ) -> Result<Reply, CallError> {
        self.raw_within(service, command, body.to_string(), limit)
            .await
    }
}

impl AppClient for SupervisedClient {
    async fn app_call<'a>(
        &'a self,
        service: &'a str,
        command: &'a str,
        body: &'a Value,
        limit: Duration,
    ) -> Result<Reply, CallError> {
        let (rc, body, error) = self
            .call_with_headers_raw_within(
                service,
                command,
                &BTreeMap::new(),
                &body.to_string(),
                limit,
            )
            .await
            .map_err(CallError::transport)?;
        Ok(Reply { rc, body, error })
    }
}

#[derive(Debug)]
pub enum AppCallError {
    Transport(CallError),
    LaunchFailed(String),
    DidNotAppear,
    Refused {
        code: String,
        message: String,
        reply: Reply,
    },
}

impl std::fmt::Display for AppCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(f),
            Self::LaunchFailed(message) => write!(f, "could not launch app: {message}"),
            Self::DidNotAppear => f.write_str("app did not appear before the wait limit"),
            Self::Refused { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for AppCallError {}

impl AppCallError {
    /// Adapt to an existing effect/reply channel without losing refusals or
    /// the transport's distinction between unsent and uncertain outcomes.
    pub fn into_result(self) -> Result<Reply, CallError> {
        match self {
            Self::Transport(error) => Err(error),
            Self::Refused { reply, .. } => Ok(reply),
            error => Ok(Reply {
                rc: 10,
                body: json!({"error_code":"NOT_RUNNING","message":error.to_string()}).to_string(),
                error: None,
            }),
        }
    }
}

fn missing(reply: &Reply, service: &str) -> bool {
    reply
        .error
        .as_deref()
        .is_some_and(|error| error == format!("Service '{service}' not found"))
}

fn checked(reply: Reply) -> Result<Reply, AppCallError> {
    if reply.rc == 0 && reply.error.is_none() {
        return Ok(reply);
    }
    let value: Value = serde_json::from_str(&reply.body).unwrap_or(Value::Null);
    Err(AppCallError::Refused {
        code: value["error_code"].as_str().unwrap_or("REFUSED").into(),
        message: value["message"]
            .as_str()
            .or(reply.error.as_deref())
            .unwrap_or(&reply.body)
            .into(),
        reply,
    })
}

/// `command` names its service (`email.compose` → `email`). After an absent
/// service, launch `app_id` once and wait at most `wait_limit` for registration.
/// Retrying an absent service is safe: the broker delivered no command.
pub async fn call_app(
    client: &impl AppClient,
    app_id: &str,
    command: &str,
    body: Value,
    wait_limit: Duration,
) -> Result<Reply, AppCallError> {
    call_app_with_limit(
        client,
        app_id,
        command,
        body,
        wait_limit,
        Duration::from_secs(30),
    )
    .await
}

/// As `call_app`, with a separate deadline for each delivered command. The
/// registration wait never cuts short a command which may already be running.
pub async fn call_app_with_limit(
    client: &impl AppClient,
    app_id: &str,
    command: &str,
    body: Value,
    wait_limit: Duration,
    limit: Duration,
) -> Result<Reply, AppCallError> {
    let service = command.split('.').next().unwrap_or(command);
    let first = client
        .app_call(service, command, &body, limit)
        .await
        .map_err(AppCallError::Transport)?;
    if !missing(&first, service) {
        return checked(first);
    }
    match client
        .app_call("apps", "apps.launch", &json!({"id":app_id}), limit)
        .await
    {
        Ok(reply) => {
            checked(reply).map_err(|error| AppCallError::LaunchFailed(error.to_string()))?;
        }
        // A lost launch reply may already have started the app. Waiting is
        // safe; launching it again would not be.
        Err(error) if error.outcome_unknown => {}
        Err(error) => return Err(AppCallError::LaunchFailed(error.to_string())),
    }
    let deadline = tokio::time::Instant::now() + wait_limit;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(AppCallError::DidNotAppear);
        }
        tokio::time::sleep(Duration::from_millis(250).min(remaining)).await;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(AppCallError::DidNotAppear);
        }
        let reply = tokio::time::timeout(limit, client.app_call(service, command, &body, limit))
            .await
            .map_err(|_| {
                AppCallError::Transport(CallError {
                    message: "app call timed out; outcome unknown".into(),
                    outcome_unknown: true,
                })
            })?
            .map_err(AppCallError::Transport)?;
        if !missing(&reply, service) {
            return checked(reply);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(rc: u8, body: Value) -> Result<Reply, CallError> {
        Ok(Reply {
            rc,
            body: body.to_string(),
            error: None,
        })
    }

    fn absent() -> Result<Reply, CallError> {
        Ok(Reply {
            rc: 10,
            error: Some("Service 'email' not found".into()),
            ..Reply::default()
        })
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn missing_launches_once_then_retries_the_same_body() {
        let client = Handle::scripted(vec![
            absent(),
            reply(0, json!({})),
            absent(),
            reply(0, json!({"ok":true})),
        ]);
        let body = json!({"attachments":["/home/user/report.pdf"]});
        let result = runtime()
            .block_on(call_app(
                &client,
                "dev.mixos.email",
                "email.compose",
                body.clone(),
                Duration::from_secs(2),
            ))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&result.body).unwrap()["ok"],
            true
        );
        let calls: Vec<_> = client
            .log()
            .into_iter()
            .filter_map(|logged| match logged {
                crate::Logged::Call {
                    service,
                    command,
                    body,
                } => Some((
                    service,
                    command,
                    serde_json::from_str::<Value>(&body).unwrap(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            calls.iter().filter(|(_, c, _)| c == "apps.launch").count(),
            1
        );
        assert_eq!(calls[0].2, body);
        assert_eq!(calls[3].2, body);
    }

    #[test]
    fn refusal_passthrough_and_no_launch_on_uncertain_outcome() {
        let body = json!({"error_code":"BUSY","message":"already composing"});
        let client = Handle::scripted(vec![reply(10, body.clone())]);
        let error = runtime()
            .block_on(call_app(
                &client,
                "dev.mixos.email",
                "email.compose",
                json!({}),
                Duration::from_secs(1),
            ))
            .unwrap_err();
        assert!(
            matches!(&error, AppCallError::Refused { code, message, .. } if code == "BUSY" && message == "already composing")
        );
        assert_eq!(
            serde_json::from_str::<Value>(&error.into_result().unwrap().body).unwrap(),
            body
        );
        assert_eq!(client.log().len(), 1);
        let client = Handle::scripted(vec![Err(CallError::from("lost reply"))]);
        assert!(matches!(
            runtime().block_on(call_app(
                &client,
                "dev.mixos.email",
                "email.compose",
                json!({}),
                Duration::ZERO
            )),
            Err(AppCallError::Transport(_))
        ));
        assert_eq!(client.log().len(), 1);
    }

    #[test]
    fn deadline_during_a_delivered_call_is_uncertain() {
        struct SlowApp(std::sync::atomic::AtomicUsize);

        impl AppClient for SlowApp {
            async fn app_call<'a>(
                &'a self,
                _service: &'a str,
                _command: &'a str,
                _body: &'a Value,
                _limit: Duration,
            ) -> Result<Reply, CallError> {
                match self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                    0 => absent(),
                    1 => reply(0, json!({})),
                    _ => std::future::pending().await,
                }
            }
        }

        let client = SlowApp(std::sync::atomic::AtomicUsize::new(0));
        let error = runtime()
            .block_on(call_app_with_limit(
                &client,
                "dev.mixos.email",
                "email.compose",
                json!({}),
                Duration::from_secs(1),
                Duration::from_millis(20),
            ))
            .unwrap_err();
        assert!(matches!(
            error,
            AppCallError::Transport(CallError {
                outcome_unknown: true,
                ..
            })
        ));
        assert_eq!(client.0.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[test]
    fn timeout_and_launch_failure_are_distinct() {
        let client = Handle::scripted(vec![absent(), reply(0, json!({}))]);
        assert!(matches!(
            runtime().block_on(call_app(
                &client,
                "dev.mixos.email",
                "email.compose",
                json!({}),
                Duration::ZERO
            )),
            Err(AppCallError::DidNotAppear)
        ));
        let client = Handle::scripted(vec![absent(), reply(10, json!({"message":"no launcher"}))]);
        assert!(matches!(
            runtime().block_on(call_app(
                &client,
                "dev.mixos.email",
                "email.compose",
                json!({}),
                Duration::ZERO
            )),
            Err(AppCallError::LaunchFailed(_))
        ));
    }
}
