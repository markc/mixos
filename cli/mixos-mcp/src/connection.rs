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
    name: String,
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

impl BrokerConnection {
    pub(crate) fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            client: Mutex::new(None),
            connecting: tokio::sync::Mutex::new(()),
            name: format!("mcp-{}-{nonce:x}", std::process::id()),
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
        let connect = async {
            match configuration.resolve_endpoint().await {
                Ok(endpoint) => {
                    let mut options = configuration
                        .into_options()
                        .map_err(|error| format!("{error:#}"))?;
                    options.require_native_session = true;
                    options.incoming_capacity = Some(64);
                    // Retain the resolved locator, including one advertised by ping.
                    options.endpoint = Some(endpoint);
                    match NodedClient::connect_unix(&self.name, &url, &options, Some(provenance))
                        .await
                        .map_err(|error| error.to_string())?
                    {
                        UnixConnectOutcome::VerifiedUnix(connection) => Ok(Client::Unix {
                            connection: Arc::new(connection),
                            drain: Mutex::new(None),
                        }),
                        UnixConnectOutcome::UnverifiedTcp { .. } => {
                            Err("native Unix connection required".into())
                        }
                    }
                }
                Err(ConnectError::CandidatesMissing(paths)) => {
                    tcp_permission(std::env::var("MIXOS_MCP_ALLOW_TCP").ok().as_deref(), paths)?;
                    NodedClient::connect_with_provenance(&self.name, &url, Some(provenance))
                        .await
                        .map(|client| Client::Tcp(Box::new(client)))
                        .map_err(|error| error.to_string())
                }
                Err(error) => Err(error.to_string()),
            }
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
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(1),
                            receiver.client().respond(
                                delivery.command(),
                                10,
                                "MCP bridge has no application command port",
                            ),
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
                            let _ = tokio::time::timeout(
                                std::time::Duration::from_secs(1),
                                owner.respond(
                                    &command,
                                    10,
                                    "MCP bridge has no application command port",
                                ),
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
