// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! monitord on the Bus: the read commands, and `monitord.summary` as a
//! retained topic every few seconds. The broker is optional: monitord keeps
//! sampling and recording while noded is down, and joins when it appears.
use crate::{engine::Engine, sampler, service};
use bus::native_client::{BoundedIncomingEvent, ConnState, SupervisedClient};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

pub const SUMMARY_TOPIC: &str = "monitord.summary";
pub const SUMMARY_EVERY: Duration = Duration::from_secs(5);
/// Bus requests served at once; more are refused `busy`, not queued.
const WORKERS: usize = 8;
/// A reply the broker doesn't take within this is dropped.
const REPLY: Duration = Duration::from_secs(3);

pub async fn serve(engine: Arc<Engine>, noded_url: String) -> anyhow::Result<()> {
    let build = buildinfo::build_info!();
    let provenance = bus::RegisterProvenance::from_parts(
        build.pkg,
        build.version,
        build.git_sha,
        build.git_dirty,
        build.build_time,
        buildinfo::now_rfc3339(),
    );
    let client = Arc::new(
        SupervisedClient::connect_options("monitord", &noded_url)
            .bounded_incoming(32)
            .fatal_on_registration_rejection(true)
            .with_commands(service::manifest())
            .with_provenance(provenance)
            .start(),
    );
    let workers = Arc::new(Semaphore::new(WORKERS));
    let mut incoming = client
        .incoming_bounded()
        .ok_or_else(|| anyhow::anyhow!("native incoming already taken"))?;
    let mut state = client.subscribe_state();
    let mut summary = tokio::time::interval(SUMMARY_EVERY);
    summary.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut connected = false;
    loop {
        tokio::select! {
            changed = state.changed() => {
                if changed.is_err() { break; }
                let now = *state.borrow_and_update();
                if now == ConnState::Fatal {
                    anyhow::bail!("Bus registration rejected; another monitord holds the name");
                }
                connected = now == ConnState::Connected;
            }
            _ = summary.tick() => {
                if !connected { continue; }
                let Some(snapshot) = engine.latest() else { continue };
                if let Err(error) = publish_retained(&client, SUMMARY_TOPIC, &sampler::summary(&snapshot)).await {
                    tracing::debug!(%error, "summary publication failed");
                }
            }
            event = incoming.recv() => match event {
                None => break,
                Some(BoundedIncomingEvent::Overflow { dropped }) => {
                    tracing::warn!(dropped, "Bus request input overflow");
                }
                Some(BoundedIncomingEvent::Command(command)) => {
                    if command.is_topic_delivery() { continue; }
                    // One task per request, at most WORKERS at once, so a slow
                    // process scan never holds up the requests behind it.
                    let Ok(permit) = workers.clone().try_acquire_owned() else {
                        let busy = json!({"status":"busy","command":command.command});
                        let _ = tokio::time::timeout(REPLY, client.respond(&command, 10, &busy.to_string())).await;
                        continue;
                    };
                    let (client, engine) = (client.clone(), engine.clone());
                    tokio::spawn(async move {
                        let body: Value = serde_json::from_str(&command.body).unwrap_or_else(|_| json!({}));
                        let reply = service::dispatch(&engine, &command.command, &body).await;
                        let (rc, value) = match reply { Ok(v) => (0, v), Err(e) => (10, e) };
                        match tokio::time::timeout(REPLY, client.respond(&command, rc, &value.to_string())).await {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => tracing::warn!(%error, "Bus reply failed"),
                            Err(_) => tracing::warn!("Bus reply timed out"),
                        }
                        drop(permit);
                    });
                }
            }
        }
    }
    Ok(())
}

async fn publish_retained(
    client: &SupervisedClient,
    topic: &str,
    body: &Value,
) -> anyhow::Result<()> {
    let mut inner = bus::wire::BusMessage::new();
    inner.set("command", topic);
    inner.body = serde_json::to_string(body)?;
    let headers = BTreeMap::from([
        ("name".into(), topic.to_owned()),
        ("retain".into(), "true".into()),
    ]);
    tokio::time::timeout(
        Duration::from_secs(3),
        client.call_with_headers("noded", "topic.publish", &headers, &inner.to_wire()),
    )
    .await??;
    Ok(())
}
