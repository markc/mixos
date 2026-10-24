// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Workshop's Bus: the app connection is the shared `citizen` library
//! (promoted from this file when Prefs became its second owner); what stays
//! here is Workshop's own discovery of the services and their verbs.
#[cfg(feature = "testing")]
pub use citizen::Logged;
pub use citizen::{CallError, Delivery, Handle, Reply, macros};
use serde_json::Value;
use std::sync::{Arc, LazyLock};

/// The app name: the worker thread, the closing refusal and the
/// `workshop.ping` / `workshop.show` activation verbs.
const APP: &str = "workshop";

/// Register as `service` on the broker at `url`, also subscribed to
/// `topics` (the settings profile's change hints).
pub fn start(
    service: &str,
    url: &str,
    topics: &[String],
) -> Result<(Handle, futures::channel::mpsc::Receiver<Delivery>), String> {
    citizen::start_with_macros(APP, service, url, topics)
}

/// Is a Workshop already registered as `service`?
pub fn probe(url: &str, service: &str) -> bool {
    citizen::probe(url, service, APP)
}

/// Ask the running Workshop to show its window.
pub fn forward(url: &str, service: &str) -> Result<(), String> {
    citizen::forward(url, service, APP)
}

/// Restore and focus this process's window through compd (`comp`).
pub async fn show(handle: Handle, comp: String) -> Result<Value, String> {
    citizen::show(handle, comp, crate::model::APP_ID).await
}

#[derive(Clone)]
struct Probe {
    handle: Handle,
    permits: Arc<tokio::sync::Semaphore>,
}

async fn json_call(probe: &Probe, service: &str, verb: &str) -> Result<Value, String> {
    let _permit = acquire(probe).await?;
    json_call_within(probe, service, verb, DISCOVERY_CALL_BUDGET).await
}

async fn acquire(probe: &Probe) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
    tokio::time::timeout(
        DISCOVERY_QUEUE_BUDGET,
        probe.permits.clone().acquire_owned(),
    )
    .await
    .map_err(|_| "discovery capacity deadline exceeded".to_owned())?
    .map_err(|e| e.to_string())
}

async fn json_call_within(
    probe: &Probe,
    service: &str,
    verb: &str,
    budget: std::time::Duration,
) -> Result<Value, String> {
    let reply = tokio::time::timeout(
        budget,
        probe
            .handle
            .raw_within(service, verb, String::new(), budget),
    )
    .await
    .map_err(|_| format!("{service} {verb}: deadline exceeded (3 s)"))?
    .map_err(|e| e.to_string())?;
    if reply.rc >= 10 {
        return Err(format!(
            "rc = {}: {} {}",
            reply.rc,
            reply.error.unwrap_or_default(),
            reply.body
        ));
    }
    serde_json::from_str(&reply.body).map_err(|e| e.to_string())
}
async fn describe(handle: &Probe, service: &str) -> Result<Vec<crate::model::Verb>, String> {
    let _permit = acquire(handle).await?;
    let deadline = tokio::time::Instant::now() + DISCOVERY_CALL_BUDGET;
    let help = match json_call_within(handle, service, "HELP", DISCOVERY_CALL_BUDGET).await {
        Ok(value) => crate::model::parse_verbs(&value),
        Err(error) => Err(error),
    };
    match help {
        Ok(verbs) => Ok(verbs),
        Err(help_error) => match json_call_within(
            handle,
            service,
            "app.describe",
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await
        {
            Ok(value) => crate::model::parse_verbs(&value),
            Err(error) => Err(format!("HELP: {help_error}\napp.describe: {error}")),
        },
    }
}
/// Discovery never inherits the mutation lane's 30 second deadline.
pub const DISCOVERY_CALL_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);
pub const DISCOVERY_QUEUE_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
pub const DISCOVERY_CONCURRENCY: usize = 8;
/// Twelve native requests across discovery leave four of citizen's sixteen
/// slots for other work. Six are reserved locally, six shared remotely.
pub const LOCAL_CALL_CONCURRENCY: usize = 6;
pub const REMOTE_CALL_CONCURRENCY: usize = 6;
// Shared across refresh generations, including abandoned description reads.
static LOCAL_PERMITS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(LOCAL_CALL_CONCURRENCY)));
static REMOTE_PERMITS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(REMOTE_CALL_CONCURRENCY)));

async fn node_snapshot(handle: &Probe, name: String, local: bool) -> crate::model::Snapshot {
    use crate::model::{Node, Snapshot};
    let broker = if local {
        "noded".to_owned()
    } else {
        format!("noded.{name}.bus")
    };
    let (info, listed) = futures::join!(
        json_call(handle, &broker, "noded.info"),
        json_call(handle, &broker, "noded.list")
    );
    let mut node = Node {
        name,
        local,
        ..Default::default()
    };
    match info {
        Ok(info) => {
            if local {
                node.name = info
                    .get("name")
                    .or_else(|| info.get("node"))
                    .and_then(Value::as_str)
                    .unwrap_or("local")
                    .to_owned();
            }
            node.info = info;
        }
        Err(error) => node.error = Some(format!("noded.info: {error}")),
    }
    let mut snapshot = Snapshot::default();
    match listed.and_then(|value| {
        serde_json::from_value::<Vec<bus::ServiceInfo>>(value).map_err(|e| e.to_string())
    }) {
        Ok(listed) => {
            for identity in listed {
                let route = if local {
                    identity.name.clone()
                } else {
                    format!("{}.{name}.bus", identity.name, name = node.name)
                };
                node.services.push(route.clone());
                snapshot.identities.insert(route, identity);
            }
            if !node.services.contains(&broker) {
                node.services.push(broker.clone());
            }
            if let Some(identity) = node
                .info
                .get("noded")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
            {
                snapshot.identities.insert(broker, identity);
            }
            // Identity and membership are usable before descriptions finish.
            for route in &node.services {
                snapshot.services.insert(route.clone(), Ok(Vec::new()));
            }
        }
        Err(error) => {
            node.error = Some(format!("noded.list: {error}"));
            if local {
                snapshot.error = node.error.clone();
            }
        }
    }
    snapshot.nodes.push(node);
    snapshot
}

enum Update {
    Peers(Result<Vec<String>, String>),
    Node(crate::model::Snapshot, bool),
}

async fn discover_node(
    handle: Probe,
    name: String,
    local: bool,
    tx: futures::channel::mpsc::UnboundedSender<Update>,
) {
    use futures::{StreamExt, stream};
    let mut snapshot = node_snapshot(&handle, name, local).await;
    let _ = tx.unbounded_send(Update::Node(snapshot.clone(), false));
    // Every queued read has a bounded acquisition, then its own call budget.
    let names = snapshot.nodes[0].services.clone();
    for result in snapshot.services.values_mut() {
        *result = Err("description deadline exceeded (3 s)".into());
    }
    let mut descriptions = stream::iter(names.into_iter().map(|name| {
        let handle = handle.clone();
        async move {
            let result = describe(&handle, &name).await;
            (name, result)
        }
    }))
    .buffer_unordered(DISCOVERY_CONCURRENCY);
    while let Some((name, result)) = descriptions.next().await {
        snapshot.services.insert(name, result);
    }
    let _ = tx.unbounded_send(Update::Node(snapshot, true));
}

/// Publish local membership immediately, then remote results as they arrive.
/// Local discovery owns a separate slot; remote calls can never starve it.
pub async fn discover_incremental(
    handle: Handle,
    mut changed: impl FnMut(crate::model::Snapshot),
) -> crate::model::Snapshot {
    use crate::model::{Progress, Snapshot};
    use futures::{StreamExt, stream};
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let local_probe = Probe {
        handle: handle.clone(),
        permits: LOCAL_PERMITS.clone(),
    };
    let handle = Probe {
        handle,
        permits: REMOTE_PERMITS.clone(),
    };
    let local = discover_node(local_probe, "local".into(), true, tx.clone());
    let remote_tx = tx.clone();
    let remote = async move {
        let tx = remote_tx;
        let peers = json_call(&handle, "noded", "noded.peers")
            .await
            .map(|value| crate::model::peers(&value));
        let names = peers.clone().unwrap_or_default();
        let _ = tx.unbounded_send(Update::Peers(peers));
        stream::iter(
            names
                .into_iter()
                .map(|name| discover_node(handle.clone(), name, false, tx.clone())),
        )
        .buffer_unordered(DISCOVERY_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    };
    let mut snapshot = Snapshot {
        progress: Some(Progress {
            total: 1,
            ..Default::default()
        }),
        ..Default::default()
    };
    let collect = async {
        while let Some(update) = rx.next().await {
            match update {
                Update::Peers(peers) => match peers {
                    Ok(peers) => {
                        snapshot.progress.as_mut().unwrap().total = peers.len() + 1;
                        snapshot.peers = peers;
                    }
                    Err(error) => snapshot.peer_error = Some(error),
                },
                Update::Node(part, done) => {
                    if part.error.is_some() {
                        snapshot.error = part.error.clone();
                    }
                    let node = part.nodes.into_iter().next().unwrap();
                    snapshot
                        .nodes
                        .retain(|old| old.name != node.name || old.local != node.local);
                    snapshot.nodes.push(node);
                    snapshot
                        .nodes
                        .sort_by(|a, b| b.local.cmp(&a.local).then(a.name.cmp(&b.name)));
                    snapshot.identities.extend(part.identities);
                    snapshot.services.extend(part.services);
                    if done {
                        snapshot.progress.as_mut().unwrap().completed += 1;
                    }
                }
            }
            // Until local membership has answered, an empty/remote-only update
            // must not overwrite the engine's last good local snapshot.
            if snapshot.nodes.iter().any(|node| node.local) {
                changed(snapshot.clone());
            }
        }
    };
    let work = async {
        futures::join!(local, remote);
        drop(tx);
    };
    futures::join!(work, collect);
    snapshot.progress.as_mut().unwrap().finished = true;
    changed(snapshot.clone());
    snapshot
}

/// Compatibility for callers that only need the final result.
pub async fn discover(handle: Handle) -> crate::model::Snapshot {
    discover_incremental(handle, |_| {}).await
}
