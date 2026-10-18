// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Lazy native Bus connection. Reconnection only serves a new request;
//! an in-flight command is never replayed.

use bus::native_client::{BrokerAccount, NodedClient, UnixConnectOutcome, VerifiedConnection};
use std::sync::{Arc, Mutex};

pub(crate) enum Client {
    Unix {
        connection: Arc<VerifiedConnection>,
        drain: Mutex<Option<tokio::task::AbortHandle>>,
    },
    Tcp(NodedClient),
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

fn broker_account(name: &str) -> Result<BrokerAccount, String> {
    let name = std::ffi::CString::new(name).map_err(|_| "invalid broker account")?;
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0_u8; 65536];
    let mut result = std::ptr::null_mut();
    // SAFETY: the entry and backing buffer remain live until uid/gid are copied.
    let rc = unsafe {
        libc::getpwnam_r(
            name.as_ptr(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return Err("configured broker account is unavailable".into());
    }
    // SAFETY: getpwnam_r succeeded and returned a non-null result.
    let entry = unsafe { entry.assume_init() };
    Ok(BrokerAccount {
        uid: entry.pw_uid,
        gid: entry.pw_gid,
    })
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
        let connect = async {
            let url = bus::noded_url();
            let socket = std::env::var_os("MIXOS_BUS_SOCKET").filter(|s| !s.is_empty());
            let account = std::env::var("MIXOS_BROKER_ACCOUNT").ok();
            if socket.is_some() || account.is_some() {
                let account = broker_account(account.as_deref().unwrap_or("noded"))?;
                let mut options = bus::client_helpers::unix_connect_options(account)
                    .map_err(|e| e.to_string())?;
                options.endpoint = socket.map(Into::into);
                options.require_native_session = true;
                options.incoming_capacity = Some(64);
                match NodedClient::connect_unix(&self.name, &url, &options, Some(provenance))
                    .await
                    .map_err(|e| e.to_string())?
                {
                    UnixConnectOutcome::VerifiedUnix(connection) => Ok(Client::Unix {
                        connection: Arc::new(connection),
                        drain: Mutex::new(None),
                    }),
                    UnixConnectOutcome::UnverifiedTcp { .. } => {
                        Err("native Unix connection required".into())
                    }
                }
            } else {
                NodedClient::connect_with_provenance(&self.name, &url, Some(provenance))
                    .await
                    .map(Client::Tcp)
                    .map_err(|e| e.to_string())
            }
        };
        let client = tokio::time::timeout(std::time::Duration::from_secs(5), connect)
            .await
            .map_err(|_| {
                "broker connection/registration deadline elapsed; nothing was sent".to_string()
            })?
            .map_err(|e| format!("broker connect failed: {e}; nothing was sent"))?;
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
    fn invalid_broker_accounts_fail_closed() {
        assert!(broker_account("invalid\0account").is_err());
        assert!(broker_account("mixos-mcp-no-such-account").is_err());
    }
}
