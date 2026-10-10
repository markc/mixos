// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ABP feed uses the shared settings Consumer for subscription, initial
//! reads, delivery loss, reconnect generations and bounded authority retries.
//! Each appearance read includes its atomic snapshot evidence from settingsd.
use crate::status::{self, Shared};
use bus::PortReply;
use bus::native_client::{BoundedIncomingEvent, ConnState, SupervisedClient};
use serde_json::{Value, json};
use settings::appearance::AppearanceProjection;
use settings::consumer::{Consumer, Work, WorkKind};
use settings::native;
use settings::{Binding, Diagnostic, Snapshot};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub const VERBS: &[&str] = &["portald.status"];
pub const SOURCE: &str = "settingsd";
pub const FETCH_VERB: &str = "settings.appearance.get";
const FETCH_TIMEOUT: Duration = Duration::from_secs(1);
const DEREGISTER_TIMEOUT: Duration = Duration::from_secs(3);

pub fn manifest() -> Vec<bus::VerbDescriptor> {
    std::iter::once(bus::VerbDescriptor::new(
        "HELP",
        &[],
        "List served verbs",
        true,
    ))
    .chain(
        VERBS
            .iter()
            .map(|verb| bus::VerbDescriptor::new(verb, &[], "Settings portal status", true)),
    )
    .collect()
}

fn fault(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("read_failed", FETCH_VERB, message)
}

struct Read {
    snapshot: Option<Snapshot>,
    appearance: Option<AppearanceProjection>,
}

/// One bounded atomic read. A projection cannot be forwarded without matching
/// binding and snapshot identity, and the Consumer validates the snapshot.
async fn fetch(client: &SupervisedClient, binding: &Binding) -> Result<Read, Diagnostic> {
    let reply = client
        .call_typed(SOURCE, FETCH_VERB, json!({ "binding": binding }))
        .await
        .map_err(|error| fault(error.to_string()))?;
    let value = match reply {
        PortReply::Ok { rc: 0, value } => value,
        PortReply::Ok { rc, value } => return Err(fault(format!("{FETCH_VERB} rc {rc}: {value}"))),
        PortReply::AppError { rc, message } => {
            return Err(fault(format!("{FETCH_VERB} rc {rc}: {message}")));
        }
    };
    if serde_json::to_vec(&value)
        .map_err(|e| fault(e.to_string()))?
        .len()
        > settings::MAX_SNAPSHOT_BYTES + 64 * 1024
    {
        return Err(fault("Appearance response exceeds envelope budget"));
    }
    if value.get("status").and_then(Value::as_str) != Some("current") {
        return Err(fault("Authority did not return a current projection"));
    }
    let appearance: AppearanceProjection = serde_json::from_value(
        value
            .get("appearance")
            .cloned()
            .ok_or_else(|| fault("Missing appearance"))?,
    )
    .map_err(|error| fault(error.to_string()))?;
    appearance.validate()?;
    let snapshot: Snapshot = serde_json::from_value(
        value
            .get("snapshot")
            .cloned()
            .ok_or_else(|| fault("Missing snapshot evidence"))?,
    )
    .map_err(|error| fault(error.to_string()))?;
    if &appearance.binding != binding || &snapshot.binding != binding {
        return Err(Diagnostic::new(
            "wrong_target",
            "binding",
            "Appearance binding differs",
        ));
    }
    let expected = AppearanceProjection::from_snapshot(&snapshot, appearance.accent)?;
    if expected != appearance {
        return Err(fault(
            "Appearance differs from its atomic snapshot evidence",
        ));
    }
    Ok(Read {
        snapshot: Some(snapshot),
        appearance: Some(appearance),
    })
}

async fn execute(client: Arc<SupervisedClient>, work: Work) -> Result<Read, Diagnostic> {
    if work.kind() == WorkKind::Subscribe {
        return native::execute(&client, &work).await.map(|snapshot| Read {
            snapshot,
            appearance: None,
        });
    }
    if native::live_generation(&client) != Some(work.generation()) {
        return Err(fault("Connection changed before appearance read"));
    }
    let result = tokio::time::timeout(FETCH_TIMEOUT, fetch(&client, work.binding()))
        .await
        .map_err(|_| Diagnostic::new("read_timeout", FETCH_VERB, "Appearance read timed out"))?;
    if native::live_generation(&client) != Some(work.generation()) {
        return Err(fault("Connection changed during appearance read"));
    }
    result
}

struct Job {
    work: Work,
    future: Pin<Box<dyn Future<Output = Result<Read, Diagnostic>> + Send>>,
}

async fn retry_after(delay: Option<Duration>) {
    match delay {
        Some(delay) => tokio::time::sleep(delay).await,
        None => std::future::pending::<()>().await,
    }
}

pub async fn feed(
    binding: Binding,
    noded_url: String,
    projections: mpsc::Sender<AppearanceProjection>,
    status: Shared,
) -> anyhow::Result<()> {
    let mut consumer =
        Consumer::for_shell(binding).map_err(|error| anyhow::anyhow!(error.message))?;
    let build = buildinfo::build_info!();
    let provenance = bus::RegisterProvenance::from_parts(
        build.pkg,
        build.version,
        build.git_sha,
        build.git_dirty,
        build.build_time,
        buildinfo::now_rfc3339(),
    );
    // Lazy start never exhausts an initial attempt budget. The native supervisor
    // keeps retrying transport with capped backoff until the feed is closed.
    let client = Arc::new(
        SupervisedClient::connect_options("portald", &noded_url)
            .bounded_incoming(64)
            .fatal_on_registration_rejection(true)
            .with_verbs(manifest())
            .with_provenance(provenance)
            .start(),
    );
    let mut state = client.subscribe_state();
    let mut incoming = client
        .incoming_bounded()
        .ok_or_else(|| anyhow::anyhow!("native incoming already taken"))?;
    let mut job: Option<Job> = None;
    let result = loop {
        // Sample INITIAL state and every generation before waiting for edges.
        // Consumer::connected deduplicates already processed generations.
        let now = *state.borrow_and_update();
        if now == ConnState::Fatal {
            break Err(anyhow::anyhow!("portald registration rejected"));
        }
        match native::live_generation(&client) {
            Some(generation) => {
                consumer.connected(generation);
            }
            None if consumer.generation().is_some() => consumer.disconnected(),
            None => {}
        }
        // Replacing this future cancels superseded work without spawning another
        // worker. Status requests and lifecycle events stay responsive during reads.
        if job.as_ref().map(|job| &job.work) != consumer.current_work() {
            job = consumer.current_work().cloned().map(|work| Job {
                future: Box::pin(execute(client.clone(), work.clone())),
                work,
            });
        }
        let retry = consumer.retry_delay();
        tokio::select! {
            _ = projections.closed() => break Ok(()),
            changed = state.changed() => {
                if changed.is_err() { break Ok(()); }
            }
            _ = retry_after(retry) => { consumer.retry(); }
            result = async { job.as_mut().expect("enabled job").future.as_mut().await }, if job.is_some() => {
                let completed = job.take().expect("completed job");
                let (snapshot, appearance) = match result {
                    Ok(read) => (Ok(read.snapshot), read.appearance),
                    Err(error) => (Err(error), None),
                };
                consumer.complete(&completed.work, snapshot);
                if let Some(appearance) = appearance
                    && consumer.is_confirmed()
                {
                    let matches = consumer.current().is_some_and(|snapshot| {
                        appearance.identity() == (snapshot.incarnation.as_str(), snapshot.revision, snapshot.design_revision)
                    });
                    if matches {
                        if projections.send(appearance).await.is_err() { break Ok(()); }
                        if let Some(update) = consumer.pending().cloned() {
                            consumer.acknowledge(&update);
                        }
                    } else {
                        // A delivery overtook this read. Fetch the new atomic projection.
                        consumer.refresh();
                    }
                }
                if let Some(error) = consumer.fault() {
                    status::update(&status, |s| s.settings_fault = Some(error.message.clone()));
                }
            }
            event = incoming.recv() => {
                match event {
                    None => break Ok(()),
                    Some(BoundedIncomingEvent::Overflow { dropped }) => {
                        tracing::warn!(dropped, "settings delivery loss; re-reading");
                        consumer.lost();
                    }
                    Some(BoundedIncomingEvent::Command(command)) => {
                        if command.is_topic_delivery() {
                            if let Some(delivery) = native::Decoded::from_command(consumer.binding(), &command) {
                                consumer.decoded_delivery(delivery);
                                // Retained publications also wake a fresh projection read
                                // on settingsd restart, even at an unchanged revision.
                                consumer.refresh();
                            }
                            continue;
                        }
                        let (rc, value) = match command.command.as_str() {
                            "HELP" => (0, json!(manifest())),
                            "portald.status" => {
                                let mut report = status::report(&status);
                                report["settings_generation"] = json!(consumer.generation());
                                (0, report)
                            }
                            other => (10, json!({"status": "not_served", "verb": other})),
                        };
                        let body = serde_json::to_string(&value)?;
                        if let Err(error) = client.respond(&command, rc, &body).await {
                            tracing::warn!(%error, "portald reply unavailable");
                        }
                    }
                }
            }
        }
    };
    let _ = tokio::time::timeout(DEREGISTER_TIMEOUT, client.deregister()).await;
    client.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_is_read_only() {
        let manifest = manifest();
        assert!(manifest.iter().all(|verb| verb.read_only));
        assert!(manifest.iter().any(|verb| verb.name == "portald.status"));
    }
}
