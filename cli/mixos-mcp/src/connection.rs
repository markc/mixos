// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Lazy native Bus connection. Reconnection only serves a new request;
//! an in-flight command is never replayed.
//! Unix ingress is required whenever a candidate exists. With no Unix
//! endpoint, plain TCP requires the explicit MIXOS_MCP_ALLOW_TCP=1 opt-in.

use bus::native_client::{ConnectError, NodedClient, UnixConnectOutcome, VerifiedConnection};
use std::sync::{Arc, Mutex};

pub(crate) enum Client {
    Unix {
        connection: Arc<VerifiedConnection>,
        drain: Mutex<Option<tokio::task::AbortHandle>>,
    },
    Tcp(Box<NodedClient>),
}

impl std::ops::Deref for Client {
    type Target = NodedClient;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Unix { connection, .. } => connection.client(),
            Self::Tcp(client) => client,
        }
    }
}

/// The bridge serves no application commands, but like every service it
/// answers the protocol ones: `HELP` and `app.describe` list only `HELP`.
fn bridge_reply(command: &str) -> (u8, String) {
    match command {
        "HELP" | "app.describe" => {
            let help = bus::CommandDescriptor::new(
                "HELP",
                &[],
                "List the commands this MCP bridge serves",
                true,
            );
            (0, serde_json::json!([help]).to_string())
        }
        _ => (
            10,
            serde_json::json!({
                "error_code": "UNKNOWN_COMMAND",
                "message": "MCP bridge has no application command port",
            })
            .to_string(),
        ),
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Self::Unix { drain, .. } = self
            && let Some(task) = drain.get_mut().unwrap_or_else(|e| e.into_inner()).take()
        {
            task.abort();
        }
    }
}

pub(crate) struct BrokerConnection {
    client: Mutex<Option<Arc<Client>>>,
    connecting: tokio::sync::Mutex<()>,
    /// Err when MIXOS_AGENT_SESSION is set but invalid: every Bus tool
    /// then fails with that reason.
    name: Result<String, String>,
    started_at: String,
}

fn allow_tcp(value: Option<&str>) -> bool {
    value == Some("1")
}

fn tcp_permission(value: Option<&str>, paths: Vec<std::path::PathBuf>) -> Result<(), String> {
    if allow_tcp(value) {
        Ok(())
    } else {
        Err(format!(
            "{}; TCP requires MIXOS_MCP_ALLOW_TCP=1",
            ConnectError::CandidatesMissing(paths)
        ))
    }
}

async fn finish_connect(
    name: &str,
    url: &str,
    provenance: Option<bus::RegisterProvenance>,
    tcp_opt_in: Option<&str>,
    result: Result<UnixConnectOutcome, ConnectError>,
) -> Result<Client, String> {
    match result {
        Ok(UnixConnectOutcome::VerifiedUnix(connection)) => Ok(Client::Unix {
            connection: Arc::new(connection),
            drain: Mutex::new(None),
        }),
        Ok(UnixConnectOutcome::UnverifiedTcp { .. }) => {
            Err("native Unix connection required".into())
        }
        Err(ConnectError::CandidatesMissing(paths)) => {
            tcp_permission(tcp_opt_in, paths)?;
            NodedClient::connect_with_provenance(name, url, provenance)
                .await
                .map(|client| Client::Tcp(Box::new(client)))
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// The environment variable an agent gateway sets on agents it runs. A bridge
/// started by such an agent registers under a name that carries the session,
/// so the gateway can recognise its own agents calling back over the Bus.
pub(crate) const AGENT_SESSION_ENV: &str = "MIXOS_AGENT_SESSION";

/// The Bus name for this bridge.
///
/// - No agent session: the anonymous `mcp-<pid>-<nonce hex>`.
/// - A valid session (4-12 lowercase letters or digits):
///   `mcp-s-<session>-<pid hex><4 nonce hex>`, at most 29 bytes, within
///   noded's 31-byte rule. The nonce keeps bridges in separate PID
///   namespaces apart. The gateway matches the prefix `mcp-s-<session>-`.
/// - An invalid session is an error, never a silent anonymous name: a
///   misconfigured gateway must not lose its self-check quietly.
pub(crate) fn broker_name(session: Option<&str>, pid: u32, nonce: u128) -> Result<String, String> {
    match session {
        None => Ok(format!("mcp-{pid}-{nonce:x}")),
        Some(s)
            if (4..=12).contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) =>
        {
            Ok(format!("mcp-s-{s}-{pid:x}{:04x}", nonce & 0xffff))
        }
        Some(s) => Err(format!(
            "{AGENT_SESSION_ENV}={s:?} is not a session id (4-12 lowercase letters or digits); \
             refusing to connect to the Bus"
        )),
    }
}

impl BrokerConnection {
    pub(crate) fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        // Only a genuinely unset variable means "not an agent": empty or
        // non-UTF-8 values go through validation and fail closed.
        let session = std::env::var_os(AGENT_SESSION_ENV)
            .map(|value| value.to_string_lossy().into_owned());
        let name = broker_name(session.as_deref(), std::process::id(), nonce);
        if let Err(error) = &name {
            tracing::error!("{error}");
        }
        Self {
            client: Mutex::new(None),
            connecting: tokio::sync::Mutex::new(()),
            name,
            started_at: buildinfo::now_rfc3339(),
        }
    }

    fn live_client(&self) -> Option<Arc<Client>> {
        self.client
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|client| client.is_connected())
            .cloned()
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.live_client().is_some()
    }

    pub(crate) async fn get(&self) -> Result<Arc<Client>, String> {
        if let Some(client) = self.live_client() {
            return Ok(client);
        }
        let name = self.name.as_deref().map_err(|e| e.to_string())?;
        let _connecting = self.connecting.lock().await;
        if let Some(client) = self.live_client() {
            return Ok(client);
        }
        let previous = self.client.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(previous) = previous {
            previous.close().await;
        }
        let build = buildinfo::build_info!();
        let provenance = bus::RegisterProvenance {
            binary: Some("mixos-mcp".into()),
            version: Some(build.version.into()),
            git_sha: Some(build.git_sha.into()),
            git_dirty: Some(build.git_dirty),
            build_time: Some(build.build_time.into()),
            pid: Some(std::process::id()),
            started_at: Some(self.started_at.clone()),
            ..Default::default()
        };
        let configuration = bus::client_helpers::BrokerEnvironment::capture()
            .configuration()
            .map_err(|error| format!("broker discovery failed: {error:#}; nothing was sent"))?;
        let url = configuration.url.clone();
        let candidates = configuration
            .endpoint_candidates()
            .map_err(|error| error.to_string())?
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let tcp_opt_in = std::env::var("MIXOS_MCP_ALLOW_TCP").ok();
        let connect = async {
            let result = match configuration.resolve_endpoint().await {
                Ok(endpoint) => {
                    let mut options = configuration
                        .into_options()
                        .map_err(|error| format!("{error:#}"))?;
                    options.require_native_session = true;
                    options.incoming_capacity = Some(64);
                    // Retain the resolved locator, including one advertised by ping.
                    options.endpoint = Some(endpoint);
                    NodedClient::connect_unix(name, &url, &options, Some(provenance.clone())).await
                }
                Err(error) => Err(error),
            };
            finish_connect(name, &url, Some(provenance), tcp_opt_in.as_deref(), result).await
        };
        let client = tokio::time::timeout(std::time::Duration::from_secs(5), connect)
            .await
            .map_err(|_| {
                format!("broker connection/registration deadline elapsed; candidates tried: {candidates}; nothing was sent")
            })?
            .map_err(|e| format!("broker connect failed: {e}; candidates tried: {candidates}; nothing was sent"))?;
        let client = Arc::new(client);
        match client.as_ref() {
            Client::Unix { connection, drain } => {
                let receiver = Arc::clone(connection);
                let task = tokio::spawn(async move {
                    while let Some(delivery) = receiver.recv_shared().await {
                        let (rc, body) = bridge_reply(&delivery.command().command);
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(1),
                            receiver.client().respond(delivery.command(), rc, &body),
                        )
                        .await;
                    }
                });
                *drain.lock().unwrap_or_else(|e| e.into_inner()) = Some(task.abort_handle());
            }
            Client::Tcp(_) => {
                if let Some(mut incoming) = client.incoming_async().await {
                    let weak = Arc::downgrade(&client);
                    tokio::spawn(async move {
                        while let Some(command) = incoming.recv().await {
                            let Some(owner) = weak.upgrade() else {
                                break;
                            };
                            let (rc, body) = bridge_reply(&command.command);
                            let _ = tokio::time::timeout(
                                std::time::Duration::from_secs(1),
                                owner.respond(&command, rc, &body),
                            )
                            .await;
                        }
                    });
                }
            }
        }
        *self.client.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&client));
        Ok(client)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// noded's rule: 2-31 bytes, lowercase first, then lowercase/digit/'-'.
    fn noded_accepts(name: &str) -> bool {
        (2..=31).contains(&name.len())
            && name.as_bytes()[0].is_ascii_lowercase()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }

    #[test]
    fn agent_session_names_carry_the_session_and_fit_noded() {
        let name = broker_name(Some("k3x9q2ab7m1z"), 4_194_304, 0x1_beef).unwrap();
        assert_eq!(name, "mcp-s-k3x9q2ab7m1z-400000beef");
        assert!(noded_accepts(&name), "{name}");
        assert!(name.starts_with("mcp-s-k3x9q2ab7m1z-"));
        // The longest session with the largest Linux pid still fits.
        let name = broker_name(Some("zzzzzzzzzzzz"), 4_194_304, u128::MAX).unwrap();
        assert!(noded_accepts(&name), "{name}");
        // The same pid in another PID namespace still gets a different name.
        assert_ne!(
            broker_name(Some("abcd"), 1, 0x0001).unwrap(),
            broker_name(Some("abcd"), 1, 0x0002).unwrap()
        );
        let anonymous = broker_name(None, 1234, 0xabc).unwrap();
        assert_eq!(anonymous, "mcp-1234-abc");
        assert!(noded_accepts(&anonymous));
    }

    #[test]
    fn a_session_prefix_never_matches_a_longer_session() {
        let longer = broker_name(Some("abcd1"), 9, 0).unwrap();
        assert!(!longer.starts_with("mcp-s-abcd-"), "{longer}");
    }

    #[test]
    fn invalid_agent_sessions_refuse_the_bus_instead_of_going_anonymous() {
        for bad in [
            "abc",
            "toolongsession1",
            "Upper1",
            "has-dash",
            "a.b.c.d",
            "sp ace",
            "",
            "abcd\u{fffd}",
        ] {
            let error = broker_name(Some(bad), 77, 0x1f).unwrap_err();
            assert!(error.contains(AGENT_SESSION_ENV), "{bad}: {error}");
        }
    }

    #[tokio::test]
    async fn an_invalid_session_fails_every_bus_connection() {
        let connection = BrokerConnection {
            client: Mutex::new(None),
            connecting: tokio::sync::Mutex::new(()),
            name: broker_name(Some("BAD"), 1, 0),
            started_at: String::new(),
        };
        let error = connection.get().await.err().expect("must refuse");
        assert!(error.contains("refusing to connect"), "{error}");
    }

    #[test]
    fn bridge_answers_protocol_commands_and_refuses_the_rest() {
        for command in ["HELP", "app.describe"] {
            let (rc, body) = bridge_reply(command);
            assert_eq!(rc, 0);
            let list: Vec<bus::CommandDescriptor> = serde_json::from_str(&body).unwrap();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].name, "HELP");
            assert!(list[0].read_only);
        }
        let (rc, body) = bridge_reply("mcp.anything");
        assert_eq!(rc, 10);
        let refusal: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(refusal["error_code"], "UNKNOWN_COMMAND");
    }

    #[tokio::test]
    async fn missing_candidates_with_opt_in_connect_over_tcp() {
        let broker = test_broker::Broker::start();
        let mut options = broker.options();
        options.endpoint = None;
        options.discover_by_ping = false;
        let result = NodedClient::connect_unix("mcp-tcp-test", &broker.url, &options, None).await;
        assert!(matches!(result, Err(ConnectError::CandidatesMissing(_))));
        let client = finish_connect("mcp-tcp-test", &broker.url, None, Some("1"), result)
            .await
            .ok()
            .unwrap();
        assert!(matches!(client, Client::Tcp(_)));
        assert_eq!(
            client
                .call("noded", "noded.ping", serde_json::Value::Null)
                .await
                .unwrap()["pong"],
            true
        );
        client.close().await;
    }

    #[tokio::test]
    async fn existing_failed_socket_never_takes_tcp_even_with_opt_in() {
        let broker = test_broker::Broker::start();
        let directory = tempfile::tempdir().unwrap();
        let endpoint = directory.path().canonicalize().unwrap().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&endpoint).unwrap());
        let mut options = broker.options();
        options.endpoint = Some(endpoint.clone());
        options.allow_unverified_tcp_fallback = true;
        let result =
            NodedClient::connect_unix("mcp-failed-test", &broker.url, &options, None).await;
        let error = finish_connect("mcp-failed-test", &broker.url, None, Some("1"), result)
            .await
            .err()
            .unwrap();
        assert!(error.contains(&endpoint.display().to_string()));
        assert!(error.contains("refused"));
    }

    #[test]
    fn tcp_requires_explicit_opt_in_and_reports_candidates() {
        let paths = vec![
            "/tmp/example/first.sock".into(),
            "/tmp/example/second.sock".into(),
        ];
        for value in [None, Some(""), Some("0"), Some("true")] {
            let error = tcp_permission(value, paths.clone()).unwrap_err();
            assert!(error.contains("MIXOS_MCP_ALLOW_TCP=1"));
            for path in &paths {
                assert!(error.contains(&path.display().to_string()));
            }
        }
        assert!(tcp_permission(Some("1"), paths).is_ok());
    }
}
