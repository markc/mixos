// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

use crate::knowledge_settings::{IndexdSettings, IndexdTransport};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

#[derive(Clone)]
pub(super) enum Transport {
    Bus {
        client: Arc<crate::connection::Client>,
        target: String,
        timeout: Duration,
    },
    Ssh {
        state: Arc<tokio::sync::Mutex<Bridge>>,
        timeout: Duration,
    },
}

impl Transport {
    pub(super) async fn connect(settings: IndexdSettings) -> Result<Self> {
        anyhow::ensure!(
            settings.call_timeout_secs > 0,
            "indexd call_timeout_secs must be positive"
        );
        let timeout = Duration::from_secs(settings.call_timeout_secs);
        match settings.transport {
            IndexdTransport::Bus => {
                anyhow::ensure!(!settings.target.trim().is_empty(), "indexd target is empty");
                let client = crate::connection::BrokerConnection::new()
                    .get()
                    .await
                    .map_err(anyhow::Error::msg)?;
                Ok(Self::Bus {
                    client,
                    target: settings.target,
                    timeout,
                })
            }
            IndexdTransport::Ssh => {
                anyhow::ensure!(
                    !settings.ssh_host.is_empty() && !settings.ssh_host.starts_with('-'),
                    "indexd ssh_host must name a host"
                );
                anyhow::ensure!(
                    !settings.ssh_command.is_empty() && !settings.ssh_command[0].is_empty(),
                    "indexd ssh_command is empty"
                );
                Ok(Self::Ssh {
                    state: Arc::new(tokio::sync::Mutex::new(Bridge::new(settings))),
                    timeout,
                })
            }
        }
    }

    pub(super) fn is_connected(&self) -> bool {
        match self {
            Self::Bus { client, .. } => client.is_connected(),
            Self::Ssh { .. } => true, // The next call repairs an exited bridge.
        }
    }

    pub(super) async fn call(&self, verb: &str, body: &Value) -> Result<Value> {
        let limit = match self {
            Self::Bus { timeout, .. } | Self::Ssh { timeout, .. } => *timeout,
        };
        // Queue admission has its own bound; callers receive the full call budget
        // once they own the serial bridge, regardless of preceding requests.
        let mut bridge = if let Self::Ssh { state, .. } = self {
            Some(
                tokio::time::timeout(limit, state.lock())
                    .await
                    .with_context(|| {
                        format!(
                            "indexd bridge queue wait exceeded {}s; request not delivered",
                            limit.as_secs()
                        )
                    })?,
            )
        } else {
            None
        };
        tokio::time::timeout(limit, async {
            match self {
                Self::Bus { client, target, .. } => {
                    let (rc, text, error) = client
                        .call_with_headers_raw_within(
                            target,
                            verb,
                            &Default::default(),
                            &body.to_string(),
                            limit,
                        )
                        .await?;
                    if verb == "indexd.stats"
                        && rc == 10
                        && text.is_empty()
                        && error.as_deref() == Some(&format!("Service '{target}' not found"))
                    {
                        anyhow::bail!("native indexd service is not registered on the MixOS Bus");
                    }
                    if rc >= 10 {
                        // Keep raw text and header precedence exactly as Bus call().
                        let mut message = bus::BusMessage::new().with_body(&text);
                        if let Some(error) = error {
                            message.set("error", &error);
                        }
                        anyhow::bail!("{}", message.error_message());
                    }
                    let value = if text.is_empty() {
                        Value::Null
                    } else {
                        serde_json::from_str(&text).unwrap_or(Value::String(text))
                    };
                    reply_body(0, value, None)
                }
                Self::Ssh { .. } => {
                    bridge
                        .as_mut()
                        .expect("SSH lock acquired")
                        .call(verb, body)
                        .await
                }
            }
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "indexd call timed out after {}s; outcome unknown",
                limit.as_secs()
            )
        })?
    }

    pub(super) async fn shutdown(&self) {
        if let Self::Ssh { state, .. } = self {
            let Ok(mut bridge) =
                tokio::time::timeout(Duration::from_millis(500), state.lock()).await
            else {
                return;
            };
            let session = bridge.session.take();
            drop(bridge);
            if let Some(mut session) = session {
                // EOF lets the bridge finish normally. Bound shutdown for a wedged peer.
                drop(session.input.take());
                if tokio::time::timeout(Duration::from_secs(1), session.child.wait())
                    .await
                    .is_err()
                {
                    let _ = session.child.kill().await;
                }
            }
        }
    }
}

// Use the native client's application error convention in both modes.
fn reply_body(rc: i64, body: Value, header_error: Option<&str>) -> Result<Value> {
    if rc != 0 {
        let mut message = bus::BusMessage::new().with_body(&body.to_string());
        if let Some(error) = header_error {
            message.set("error", error);
        }
        anyhow::bail!("{}", message.error_message());
    }
    if let Some(error) = body.get("error").and_then(Value::as_str) {
        anyhow::bail!("indexd error: {error}");
    }
    Ok(body)
}

pub(super) struct Bridge {
    settings: IndexdSettings,
    session: Option<Session>,
    next_id: u64,
    #[cfg(test)]
    direct_command: bool,
}

impl Bridge {
    fn new(settings: IndexdSettings) -> Self {
        Self {
            settings,
            session: None,
            next_id: 0,
            #[cfg(test)]
            direct_command: false,
        }
    }

    fn spawn(&self) -> Result<Session> {
        let mut command = Command::new("ssh");
        command
            .args(["-T", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=30"])
            .arg(&self.settings.ssh_host)
            .args(&self.settings.ssh_command);
        // Only unit tests can bypass SSH; there is no production environment hook.
        #[cfg(test)]
        if self.direct_command {
            command = Command::new(&self.settings.ssh_command[0]);
            command.args(&self.settings.ssh_command[1..]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("starting indexd bridge")?;
        let input = child.stdin.take().context("bridge stdin")?;
        let output = BufReader::new(child.stdout.take().context("bridge stdout")?);
        let mut stderr = child.stderr.take().context("bridge stderr")?;
        let diagnostic = Arc::new(Mutex::new(VecDeque::new()));
        let buffer = diagnostic.clone();
        let drain = tokio::spawn(async move {
            let mut bytes = [0; 1024];
            while let Ok(n) = stderr.read(&mut bytes).await {
                if n == 0 {
                    break;
                }
                let mut buffer = buffer.lock().unwrap_or_else(|e| e.into_inner());
                buffer.extend(&bytes[..n]);
                let excess = buffer.len().saturating_sub(8192);
                buffer.drain(..excess);
            }
        });
        Ok(Session {
            child,
            input: Some(input),
            output,
            diagnostic,
            drain,
        })
    }

    async fn call(&mut self, verb: &str, body: &Value) -> Result<Value> {
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        let timeout_secs = self.settings.call_timeout_secs.saturating_sub(10).max(1);
        let request = json!({"id":id, "verb":verb, "body":body, "timeout_secs":timeout_secs})
            .to_string()
            + "\n";
        for attempt in 0..2 {
            // Owning the session across await is deliberate: cancellation or timeout
            // drops/kills it, so a late reply cannot be consumed by the next call.
            let mut session = match self.session.take() {
                Some(session) => session,
                None => match self.spawn() {
                    Ok(session) => session,
                    Err(_) if attempt == 0 => continue,
                    Err(error) => return Err(error),
                },
            };
            match session.exchange(id, &request).await {
                Ok(reply) if reply.kind.as_deref() == Some("timeout") => {
                    self.session = Some(session);
                    anyhow::bail!(
                        "indexd bridge timeout; outcome unknown: {}",
                        reply.error.as_deref().unwrap_or("request timed out")
                    );
                }
                Ok(reply) => {
                    self.session = Some(session);
                    return reply_body(reply.rc, reply.body, reply.error.as_deref());
                }
                Err(error) => {
                    // Wait for stderr EOF after killing, so short-lived diagnostics
                    // are retained without waiting on a still-running child.
                    session.stop().await;
                    let diagnostic = session
                        .diagnostic
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .iter()
                        .copied()
                        .collect::<Vec<_>>();
                    if error.delivered && !retry_after_delivery(verb) {
                        anyhow::bail!(
                            "indexd bridge failed; outcome unknown: {:#}; stderr: {}",
                            error.error,
                            String::from_utf8_lossy(&diagnostic)
                        );
                    }
                    if attempt == 1 {
                        anyhow::bail!(
                            "indexd bridge failed after one retry: {:#}; stderr: {}",
                            error.error,
                            String::from_utf8_lossy(&diagnostic)
                        );
                    }
                }
            }
        }
        unreachable!()
    }
}

#[derive(Deserialize)]
struct Reply {
    id: u64,
    rc: i64,
    #[serde(default)]
    body: Value,
    error: Option<String>,
    kind: Option<String>,
}

fn retry_after_delivery(verb: &str) -> bool {
    matches!(
        verb.strip_prefix("indexd.").unwrap_or(verb),
        "search"
            | "list"
            | "stats"
            | "stale"
            | "embed"
            | "store"
            | "update"
            | "delete"
            | "supersede"
            | "index_file"
    )
}

struct ExchangeFailure {
    delivered: bool,
    error: anyhow::Error,
}

struct Session {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    diagnostic: Arc<Mutex<VecDeque<u8>>>,
    drain: tokio::task::JoinHandle<()>,
}

impl Session {
    async fn stop(&mut self) {
        let _ = tokio::time::timeout(Duration::from_millis(500), self.child.kill()).await;
        if tokio::time::timeout(Duration::from_millis(500), &mut self.drain)
            .await
            .is_err()
        {
            self.drain.abort();
        }
    }

    async fn exchange(
        &mut self,
        id: u64,
        request: &str,
    ) -> std::result::Result<Reply, ExchangeFailure> {
        let delivery: Result<()> = async {
            anyhow::ensure!(self.child.try_wait()?.is_none(), "bridge child exited");
            let input = self.input.as_mut().context("bridge stdin closed")?;
            input
                .write_all(request.as_bytes())
                .await
                .context("writing bridge request")?;
            input.flush().await.context("flushing bridge request")?;
            Ok(())
        }
        .await;
        delivery.map_err(|error| ExchangeFailure {
            delivered: false,
            error,
        })?;
        let response: Result<Reply> = async {
            let mut line = Vec::new();
            // Match the Bus body cap while bounding a broken peer's output.
            let n = (&mut self.output)
                .take(16 * 1024 * 1024 + 1)
                .read_until(b'\n', &mut line)
                .await
                .context("reading bridge reply")?;
            anyhow::ensure!(n > 0, "bridge closed stdout");
            anyhow::ensure!(
                line.last() == Some(&b'\n') && n <= 16 * 1024 * 1024,
                "bridge reply exceeds limit or is incomplete"
            );
            let reply: Reply = serde_json::from_slice(&line).context("parsing bridge reply")?;
            anyhow::ensure!(
                reply.id == id,
                "bridge reply id mismatch: expected {id}, got {}",
                reply.id
            );
            anyhow::ensure!(
                reply.rc != -1 || reply.kind.as_deref() == Some("timeout"),
                "bridge transport failure: {}",
                reply.error.as_deref().unwrap_or("unknown error")
            );
            Ok(reply)
        }
        .await;
        response.map_err(|error| ExchangeFailure {
            delivered: true,
            error,
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.drain.abort();
        // Child's kill_on_drop covers cancellation and abnormal shutdown.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake() -> Option<(Transport, tempfile::TempDir)> {
        fake_with_timeout(1)
    }

    fn fake_with_timeout(call_timeout_secs: u64) -> Option<(Transport, tempfile::TempDir)> {
        let mix = std::env::var_os("MIXOS_TEST_MIX")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("PATH").and_then(|path| {
                    std::env::split_paths(&path)
                        .map(|p| p.join("mix"))
                        .find(|p| p.is_file())
                })
            });
        let Some(mix) = mix else {
            eprintln!("SKIP fake bridge test: set MIXOS_TEST_MIX or put Mix on PATH");
            return None;
        };
        let dir = tempfile::tempdir().unwrap();
        let settings = IndexdSettings {
            transport: IndexdTransport::Ssh,
            call_timeout_secs,
            ssh_command: vec![
                mix.to_string_lossy().into_owned(),
                format!("{}/tests/fake_bridge.mix", env!("CARGO_MANIFEST_DIR")),
                dir.path().join("starts").to_string_lossy().into_owned(),
            ],
            ..Default::default()
        };
        let mut bridge = Bridge::new(settings);
        bridge.direct_command = true;
        Some((
            Transport::Ssh {
                state: Arc::new(tokio::sync::Mutex::new(bridge)),
                timeout: Duration::from_secs(call_timeout_secs),
            },
            dir,
        ))
    }

    fn starts(dir: &tempfile::TempDir) -> u32 {
        std::fs::read_to_string(dir.path().join("starts"))
            .unwrap()
            .parse()
            .unwrap()
    }

    #[tokio::test]
    async fn ssh_round_trips_share_one_child_and_shutdown_with_eof() {
        let Some((client, dir)) = fake() else {
            return;
        };
        let a = json!({"action":"stats"});
        let b = json!({"action":"search", "query":"fixture"});
        let (first, second) = tokio::join!(
            client.call("indexd.stats", &a),
            client.call("indexd.search", &b)
        );
        assert_eq!(
            first.unwrap(),
            json!({"echo":a, "verb":"indexd.stats", "timeout_secs":1})
        );
        assert_eq!(
            second.unwrap(),
            json!({"echo":b, "verb":"indexd.search", "timeout_secs":1})
        );
        assert_eq!(starts(&dir), 1);
        client.shutdown().await;
        assert!(dir.path().join("starts.eof").exists());
    }

    #[tokio::test]
    async fn ssh_child_dying_mid_session_respawns_once() {
        let Some((client, dir)) = fake() else {
            return;
        };
        client.call("indexd.stats", &json!({})).await.unwrap();
        client
            .call("indexd.search", &json!({"mode":"die_once"}))
            .await
            .unwrap();
        client.call("indexd.stats", &json!({})).await.unwrap();
        assert_eq!(starts(&dir), 2);
        client.shutdown().await;
    }

    #[tokio::test]
    async fn ssh_persistent_failure_preserves_diagnostics_after_one_retry() {
        let Some((client, dir)) = fake() else {
            return;
        };
        let error = client
            .call("indexd.stats", &json!({"mode":"failure"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("after one retry"), "{error}");
        assert!(error.contains("fixture child failure"), "{error}");
        assert_eq!(starts(&dir), 2);
    }

    #[tokio::test]
    async fn ssh_timeout_discards_session_and_does_not_replay_call() {
        let Some((client, dir)) = fake() else {
            return;
        };
        let error = client
            .call("indexd.store", &json!({"mode":"timeout"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("timed out after 1s"), "{error}");
        // Leave enough time for a mistaken replay to start and increment the
        // counter, before an intentional subsequent call can hide it.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert_eq!(starts(&dir), 1);
        client.call("indexd.stats", &json!({})).await.unwrap();
        assert_eq!(starts(&dir), 2);
        client.shutdown().await;
    }

    #[tokio::test]
    async fn ssh_application_errors_match_bus_errors_without_respawn() {
        let Some((client, dir)) = fake() else {
            return;
        };
        let error = client
            .call("indexd.search", &json!({"mode":"error"}))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(error, "indexd error: fixture refusal");
        let body = json!({"mode":"rc"});
        let error = client
            .call("indexd.search", &body)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            bus::BusMessage::new()
                .with_body(
                    &json!({"echo":body, "verb":"indexd.search", "timeout_secs":1}).to_string()
                )
                .error_message()
        );
        assert_eq!(starts(&dir), 1);
        client.shutdown().await;
    }

    #[tokio::test]
    async fn ssh_transport_error_and_wrong_id_cannot_be_successes() {
        for mode in ["transport_error", "wrong_id"] {
            let Some((client, dir)) = fake() else {
                return;
            };
            let error = client
                .call("indexd.stats", &json!({"mode":mode}))
                .await
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(if mode == "transport_error" {
                    "fixture Bus closed"
                } else {
                    "id mismatch"
                }),
                "{error}"
            );
            assert_eq!(starts(&dir), 2);
        }
    }

    #[tokio::test]
    async fn ssh_feedback_post_delivery_failures_are_never_replayed() {
        for mode in ["die_once", "transport_error", "wrong_id"] {
            let Some((client, dir)) = fake() else {
                return;
            };
            let error = client
                .call("indexd.feedback", &json!({"mode":mode}))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("outcome unknown"), "{error}");
            assert_eq!(starts(&dir), 1, "{mode} replayed feedback");
        }
    }

    #[tokio::test]
    async fn ssh_pre_delivery_failure_replays_feedback() {
        for exited in [false, true] {
            let Some((client, dir)) = fake() else {
                return;
            };
            client
                .call("indexd.stats", &json!({"mode":"exit_after_reply"}))
                .await
                .unwrap();
            let Transport::Ssh { state, .. } = &client else {
                unreachable!()
            };
            {
                let mut bridge = state.lock().await;
                let session = bridge.session.as_mut().unwrap();
                if exited {
                    session.child.wait().await.unwrap(); // Forces exchange's try_wait path.
                } else {
                    drop(session.input.take()); // Forces the closed-stdin path.
                }
            }
            client.call("indexd.feedback", &json!({})).await.unwrap();
            assert_eq!(starts(&dir), 2);
            client.shutdown().await;
        }
    }

    #[tokio::test]
    async fn ssh_bridge_timeout_is_not_retried_for_any_verb() {
        let Some((client, dir)) = fake() else {
            return;
        };
        for verb in [
            "search",
            "list",
            "stats",
            "stale",
            "embed",
            "store",
            "update",
            "delete",
            "supersede",
            "index_file",
            "feedback",
        ] {
            let error = client
                .call(&format!("indexd.{verb}"), &json!({"mode":"bridge_timeout"}))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("outcome unknown"), "{error}");
            assert!(error.contains("fixture deadline"), "{error}");
            assert_eq!(starts(&dir), 1);
        }
        client.call("indexd.stats", &json!({})).await.unwrap();
        assert_eq!(starts(&dir), 1);
        client.shutdown().await;
    }

    #[tokio::test]
    async fn ssh_bridge_deadline_precedes_client_deadline_and_reuses_session() {
        let Some((client, dir)) = fake_with_timeout(2) else {
            return;
        };
        let error = client
            .call("indexd.feedback", &json!({"mode":"delayed_bridge_timeout"}))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("indexd bridge timeout"), "{error}");
        let reply = client.call("indexd.stats", &json!({})).await.unwrap();
        assert_eq!(reply["timeout_secs"], 1);
        assert_eq!(starts(&dir), 1);
        client.shutdown().await;

        let Some((client, _dir)) = fake_with_timeout(120) else {
            return;
        };
        let reply = client.call("indexd.stats", &json!({})).await.unwrap();
        assert_eq!(reply["timeout_secs"], 110);
        client.shutdown().await;
    }

    #[tokio::test]
    async fn ssh_stderr_flood_is_drained_and_tail_is_retained() {
        let Some((client, dir)) = fake() else {
            return;
        };
        let Transport::Ssh { state, .. } = &client else {
            unreachable!()
        };
        let error = tokio::time::timeout(Duration::from_secs(5), async {
            state
                .lock()
                .await
                .call("indexd.feedback", &json!({"mode":"flood"}))
                .await
        })
        .await
        .expect("stderr flood must not deadlock")
        .unwrap_err()
        .to_string();
        assert!(error.contains("fixture flood tail"), "{error}");
        assert!(error.len() < 9000, "diagnostic tail must be bounded");
        assert_eq!(starts(&dir), 1);
    }

    #[tokio::test]
    async fn ssh_stderr_drain_and_shutdown_lock_wait_are_bounded() {
        let Some((client, _dir)) = fake() else {
            return;
        };
        client.call("indexd.stats", &json!({})).await.unwrap();
        let Transport::Ssh { state, .. } = &client else {
            unreachable!()
        };
        let mut bridge = state.lock().await;
        let session = bridge.session.as_mut().unwrap();
        session.drain.abort();
        session.drain = tokio::spawn(std::future::pending());
        let started = tokio::time::Instant::now();
        session.stop().await;
        assert!(started.elapsed() >= Duration::from_millis(450));
        assert!(started.elapsed() < Duration::from_millis(1500));
        // The held guard models an in-flight request during shutdown.
        tokio::time::timeout(Duration::from_millis(1500), client.shutdown())
            .await
            .unwrap();
        drop(bridge);
    }

    #[tokio::test]
    async fn ssh_queue_wait_uses_configured_call_budget() {
        let Some((client, dir)) = fake_with_timeout(1) else {
            return;
        };
        let Transport::Ssh { state, .. } = &client else {
            unreachable!()
        };
        let guard = state.lock().await;
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            client.call("indexd.feedback", &json!({})),
        )
        .await
        .expect("queue wait must respect the configured budget")
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("queue wait exceeded 1s; request not delivered"),
            "{error}"
        );
        assert!(!dir.path().join("starts").exists());
        drop(guard);
        client.call("indexd.stats", &json!({})).await.unwrap();
        assert_eq!(starts(&dir), 1);
        client.shutdown().await;
    }

    #[tokio::test]
    async fn ssh_queue_wait_does_not_consume_call_deadline() {
        let Some((client, dir)) = fake() else {
            return;
        };
        let body = json!({"mode":"delay"});
        let (first, second) = tokio::join!(
            client.call("indexd.stats", &body),
            client.call("indexd.stats", &body)
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(starts(&dir), 1);
        client.shutdown().await;
    }
}
