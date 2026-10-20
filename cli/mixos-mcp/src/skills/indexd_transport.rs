// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

use crate::knowledge_settings::{IndexdSettings, IndexdTransport};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::VecDeque, process::Stdio, sync::{Arc, Mutex}, time::Duration};
use tokio::{io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader}, process::{Child, ChildStdin, ChildStdout, Command}};

#[derive(Clone)]
pub(super) enum Transport {
    Bus { client: Arc<crate::connection::Client>, target: String, timeout: Duration },
    Ssh { state: Arc<tokio::sync::Mutex<Bridge>>, timeout: Duration },
}

impl Transport {
    pub(super) async fn connect(settings: IndexdSettings) -> Result<Self> {
        anyhow::ensure!(settings.call_timeout_secs > 0, "indexd call_timeout_secs must be positive");
        let timeout = Duration::from_secs(settings.call_timeout_secs);
        match settings.transport {
            IndexdTransport::Bus => {
                anyhow::ensure!(!settings.target.trim().is_empty(), "indexd target is empty");
                let client = crate::connection::BrokerConnection::new().get().await.map_err(anyhow::Error::msg)?;
                Ok(Self::Bus { client, target: settings.target, timeout })
            }
            IndexdTransport::Ssh => {
                anyhow::ensure!(!settings.ssh_host.is_empty() && !settings.ssh_host.starts_with('-'), "indexd ssh_host must name a host");
                anyhow::ensure!(!settings.ssh_command.is_empty() && !settings.ssh_command[0].is_empty(), "indexd ssh_command is empty");
                Ok(Self::Ssh { state: Arc::new(tokio::sync::Mutex::new(Bridge::new(settings))), timeout })
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
        let limit = match self { Self::Bus { timeout, .. } | Self::Ssh { timeout, .. } => *timeout };
        tokio::time::timeout(limit, async {
            match self {
                Self::Bus { client, target, .. } => {
                    let (rc, text, error) = client.call_with_headers_raw_within(target, verb, &Default::default(), &body.to_string(), limit).await?;
                    if verb == "indexd.stats" && rc == 10 && text.is_empty() && error.as_deref() == Some(&format!("Service '{target}' not found")) {
                        anyhow::bail!("native indexd service is not registered on the MixOS Bus");
                    }
                    let value = serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text));
                    reply_body(if rc < 10 { 0 } else { i64::from(rc) }, value, error.as_deref())
                }
                Self::Ssh { state, .. } => state.lock().await.call(verb, body).await,
            }
        }).await.map_err(|_| anyhow::anyhow!("indexd call timed out after {}s; outcome unknown", limit.as_secs()))?
    }

    pub(super) async fn shutdown(&self) {
        if let Self::Ssh { state, .. } = self {
            let session = state.lock().await.session.take();
            if let Some(mut session) = session {
                // EOF lets the bridge finish normally. Bound shutdown for a wedged peer.
                drop(session.input.take());
                if tokio::time::timeout(Duration::from_secs(1), session.child.wait()).await.is_err() {
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
        if let Some(error) = header_error { message.set("error", error); }
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
        Self { settings, session: None, next_id: 0, #[cfg(test)] direct_command: false }
    }

    fn spawn(&self) -> Result<Session> {
        let mut command = Command::new("ssh");
        command.args(["-T", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=30"])
            .arg(&self.settings.ssh_host).args(&self.settings.ssh_command);
        // Only unit tests can bypass SSH; there is no production environment hook.
        #[cfg(test)]
        if self.direct_command {
            command = Command::new(&self.settings.ssh_command[0]);
            command.args(&self.settings.ssh_command[1..]);
        }
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn().context("starting indexd bridge")?;
        let input = child.stdin.take().context("bridge stdin")?;
        let output = BufReader::new(child.stdout.take().context("bridge stdout")?);
        let mut stderr = child.stderr.take().context("bridge stderr")?;
        let diagnostic = Arc::new(Mutex::new(VecDeque::new()));
        let buffer = diagnostic.clone();
        let drain = tokio::spawn(async move {
            let mut bytes = [0; 1024];
            while let Ok(n) = stderr.read(&mut bytes).await {
                if n == 0 { break; }
                let mut buffer = buffer.lock().unwrap_or_else(|e| e.into_inner());
                buffer.extend(&bytes[..n]);
                let excess = buffer.len().saturating_sub(8192);
                buffer.drain(..excess);
            }
        });
        Ok(Session { child, input: Some(input), output, diagnostic, drain })
    }

    async fn call(&mut self, verb: &str, body: &Value) -> Result<Value> {
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        let request = json!({"id":id, "verb":verb, "body":body}).to_string() + "\n";
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
                Ok(reply) => {
                    self.session = Some(session);
                    return reply_body(reply.rc, reply.body, reply.error.as_deref());
                }
                Err(error) => {
                    // Wait for stderr EOF after killing, so short-lived diagnostics
                    // are retained without waiting on a still-running child.
                    let _ = session.child.kill().await;
                    let _ = (&mut session.drain).await;
                    let diagnostic = session.diagnostic.lock().unwrap_or_else(|e| e.into_inner()).iter().copied().collect::<Vec<_>>();
                    if attempt == 1 {
                        anyhow::bail!("indexd bridge failed after one retry: {error:#}; stderr: {}", String::from_utf8_lossy(&diagnostic));
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
}

struct Session {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    diagnostic: Arc<Mutex<VecDeque<u8>>>,
    drain: tokio::task::JoinHandle<()>,
}

impl Session {
    async fn exchange(&mut self, id: u64, request: &str) -> Result<Reply> {
        anyhow::ensure!(self.child.try_wait()?.is_none(), "bridge child exited");
        let input = self.input.as_mut().context("bridge stdin closed")?;
        input.write_all(request.as_bytes()).await.context("writing bridge request")?;
        input.flush().await.context("flushing bridge request")?;
        let mut line = Vec::new();
        // Match the Bus body cap while bounding a broken peer's output.
        let n = (&mut self.output).take(16 * 1024 * 1024 + 1).read_until(b'\n', &mut line).await.context("reading bridge reply")?;
        anyhow::ensure!(n > 0, "bridge closed stdout");
        anyhow::ensure!(line.last() == Some(&b'\n') && n <= 16 * 1024 * 1024, "bridge reply exceeds limit or is incomplete");
        let reply: Reply = serde_json::from_slice(&line).context("parsing bridge reply")?;
        anyhow::ensure!(reply.id == id, "bridge reply id mismatch: expected {id}, got {}", reply.id);
        anyhow::ensure!(reply.rc != -1, "bridge transport failure: {}", reply.error.as_deref().unwrap_or("unknown error"));
        Ok(reply)
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

    fn fake() -> (Transport, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let settings = IndexdSettings {
            transport: IndexdTransport::Ssh,
            ssh_command: vec!["/opt/mixos/bin/mix".into(), format!("{}/tests/fake_bridge.mix", env!("CARGO_MANIFEST_DIR")), dir.path().join("starts").to_string_lossy().into_owned()],
            ..Default::default()
        };
        let mut bridge = Bridge::new(settings);
        bridge.direct_command = true;
        (Transport::Ssh { state: Arc::new(tokio::sync::Mutex::new(bridge)), timeout: Duration::from_secs(1) }, dir)
    }

    fn starts(dir: &tempfile::TempDir) -> u32 {
        std::fs::read_to_string(dir.path().join("starts")).unwrap().parse().unwrap()
    }

    #[tokio::test]
    async fn ssh_round_trips_share_one_child_and_shutdown_with_eof() {
        let (client, dir) = fake();
        let a = json!({"action":"stats"});
        let b = json!({"action":"search", "query":"fixture"});
        let (first, second) = tokio::join!(client.call("indexd.stats", &a), client.call("indexd.search", &b));
        assert_eq!(first.unwrap(), json!({"echo":a, "verb":"indexd.stats"}));
        assert_eq!(second.unwrap(), json!({"echo":b, "verb":"indexd.search"}));
        assert_eq!(starts(&dir), 1);
        client.shutdown().await;
        assert!(dir.path().join("starts.eof").exists());
    }

    #[tokio::test]
    async fn ssh_child_dying_mid_session_respawns_once() {
        let (client, dir) = fake();
        client.call("indexd.stats", &json!({})).await.unwrap();
        client.call("indexd.search", &json!({"mode":"die_once"})).await.unwrap();
        client.call("indexd.stats", &json!({})).await.unwrap();
        assert_eq!(starts(&dir), 2);
        client.shutdown().await;
    }

    #[tokio::test]
    async fn ssh_persistent_failure_preserves_diagnostics_after_one_retry() {
        let (client, dir) = fake();
        let error = client.call("indexd.stats", &json!({"mode":"failure"})).await.unwrap_err().to_string();
        assert!(error.contains("after one retry"), "{error}");
        assert!(error.contains("fixture child failure"), "{error}");
        assert_eq!(starts(&dir), 2);
    }

    #[tokio::test]
    async fn ssh_timeout_discards_session_and_does_not_replay_call() {
        let (client, dir) = fake();
        let error = client.call("indexd.store", &json!({"mode":"timeout"})).await.unwrap_err().to_string();
        assert!(error.contains("timed out after 1s"), "{error}");
        assert_eq!(starts(&dir), 1);
        client.call("indexd.stats", &json!({})).await.unwrap();
        assert_eq!(starts(&dir), 2);
        client.shutdown().await;
    }

    #[tokio::test]
    async fn ssh_application_errors_match_bus_errors_without_respawn() {
        let (client, dir) = fake();
        let error = client.call("indexd.search", &json!({"mode":"error"})).await.unwrap_err().to_string();
        assert_eq!(error, "indexd error: fixture refusal");
        let body = json!({"mode":"rc"});
        let error = client.call("indexd.search", &body).await.unwrap_err().to_string();
        assert_eq!(error, bus::BusMessage::new().with_body(&json!({"echo":body, "verb":"indexd.search"}).to_string()).error_message());
        assert_eq!(starts(&dir), 1);
        client.shutdown().await;
    }

    #[tokio::test]
    async fn ssh_transport_error_and_wrong_id_cannot_be_successes() {
        for mode in ["transport_error", "wrong_id"] {
            let (client, dir) = fake();
            let error = client.call("indexd.stats", &json!({"mode":mode})).await.unwrap_err().to_string();
            assert!(error.contains(if mode == "transport_error" { "fixture Bus closed" } else { "id mismatch" }), "{error}");
            assert_eq!(starts(&dir), 2);
        }
    }
}
