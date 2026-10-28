// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use bus::PortReply;
use bus::native_client::{BoundedIncomingEvent, SupervisedClient};
use serde_json::{Value, json};

pub(super) async fn attempt(
    request: &Request,
    cancel: &mut watch::Receiver<bool>,
) -> Option<Outcome> {
    attempt_at(request, cancel, &bus::client_helpers::resolve_noded_url()).await
}

async fn call(client: &SupervisedClient, command: &str, body: Value) -> Option<PortReply> {
    tokio::time::timeout(IO_BOUND, client.call_typed("files", command, body))
        .await
        .ok()?
        .ok()
}

pub(super) async fn attempt_at(
    request: &Request,
    cancel: &mut watch::Receiver<bool>,
    url: &str,
) -> Option<Outcome> {
    let mut request = request.clone();
    if request.correlation.is_none() {
        request.correlation = Some(format!("toolkit-{}", uuid::Uuid::now_v7()));
    }
    let body = match request.body() {
        Ok(body) => body,
        Err(error) => return Some(Outcome::Failed(error.message)),
    };
    let name = format!(
        "toolkit-choose-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let client = SupervisedClient::connect_options(&name, url)
        .bounded_incoming(64)
        .start();
    let result = session(&client, &request, body, cancel).await;
    let _ = tokio::time::timeout(IO_BOUND, client.close()).await;
    result
}

/// A bad accepted answer is final; only explicit provider failure advances.
fn outcome(request: &Request, value: &Value) -> Option<Outcome> {
    if value["response"] == 2 {
        None
    } else {
        Some(decode_outcome(request, value))
    }
}

struct Provider {
    incarnation: String,
    failures: u8,
}

enum Snapshot {
    Live(Value),
    Retry,
    Lost,
}

impl Provider {
    // Service discovery and list share one I/O bound, even if noded died.
    async fn snapshot(&mut self, client: &SupervisedClient) -> Snapshot {
        let (services, list) = tokio::join!(
            tokio::time::timeout(IO_BOUND, client.list_services()),
            call(client, "files.choose.list", json!({}))
        );
        if let Ok(Ok(services)) = services
            && !services.iter().any(|s| s == "files")
        {
            return Snapshot::Lost;
        }
        if let Some(PortReply::Ok { value, .. }) = list
            && let Some(incarnation) = value["incarnation"].as_str()
            && value["requests"].is_array()
        {
            if incarnation != self.incarnation {
                return Snapshot::Lost;
            }
            self.failures = 0;
            return Snapshot::Live(value);
        }
        self.failures += 1;
        if self.failures >= 2 {
            Snapshot::Lost
        } else {
            Snapshot::Retry
        }
    }
}

async fn session(
    client: &SupervisedClient,
    request: &Request,
    body: Value,
    cancel: &mut watch::Receiver<bool>,
) -> Option<Outcome> {
    let connected = tokio::time::timeout(IO_BOUND, async {
        while !client.is_connected() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    tokio::select! { result = connected => { result.ok()?; }, _ = cancelled(cancel) => return None }
    let services = tokio::time::timeout(IO_BOUND, client.list_services())
        .await
        .ok()?
        .ok()?;
    if !services.iter().any(|s| s == "files") {
        return None;
    }
    let Some(PortReply::Ok { value, .. }) = call(client, "files.choose.list", json!({})).await
    else {
        return None;
    };
    let mut provider = Provider {
        incarnation: value["incarnation"].as_str()?.to_owned(),
        failures: 0,
    };
    tokio::time::timeout(IO_BOUND, client.subscribe_topic("files.chosen"))
        .await
        .ok()?
        .ok()?;
    let mut incoming = client.incoming_bounded()?;
    if *cancel.borrow() {
        return None;
    }
    // The intent acknowledges straight away. Bound a stalled call, retaining
    // correlation to close a window even if the reply is lost or is an error.
    let reply = call(client, "files.choose", body).await;
    let id = match reply {
        Some(PortReply::Ok { value, .. }) => value["request"].as_str().map(str::to_owned),
        _ => None,
    };
    let Some(id) = id else {
        return if recover_sending(
            client,
            &mut provider,
            request.correlation.as_deref().expect("set"),
            None,
        )
        .await
        {
            None
        } else {
            Some(Outcome::Failed(
                "could not confirm the native chooser ended".into(),
            ))
        };
    };
    let mut incoming_live = true;
    let mut sweep = tokio::time::interval(Duration::from_secs(5));
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancelled(cancel) => {
                if !cancel_request(client, &id).await {
                    recover_sending(client, &mut provider, request.correlation.as_deref().expect("set"), Some(&id)).await;
                }
                return None;
            }
            event = incoming.recv(), if incoming_live => {
                match event {
                    Some(BoundedIncomingEvent::Command(event)) if event.topic() == Some("files.chosen") => {
                        if let Ok(value) = serde_json::from_str::<Value>(&event.body)
                            && value["request"].as_str() == Some(&id) {
                            return outcome(request, &value);
                        }
                    }
                    None => incoming_live = false,
                    _ => {}
                }
            }
            _ = sweep.tick() => {
                match provider.snapshot(client).await {
                    Snapshot::Lost => {
                        // One bounded best-effort cancel on loss. Fallback
                        // cannot depend on an unreachable files service.
                        if client.is_connected() {
                            let _ = cancel_request(client, &id).await;
                        }
                        return None;
                    }
                    Snapshot::Retry => continue,
                    Snapshot::Live(_) => {}
                }
                match call(client, "files.choose.result", json!({"request":id})).await {
                    Some(PortReply::Ok { value, .. }) if value["state"] == "done" && value["outcome"]["request"].as_str() == Some(&id) => return outcome(request, &value["outcome"]),
                    Some(PortReply::AppError { .. }) => {
                        return if recover_sending(client, &mut provider,
                            request.correlation.as_deref().expect("set"), Some(&id)).await {
                            None
                        } else {
                            Some(Outcome::Failed("could not confirm the native chooser ended".into()))
                        };
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn cancel_request(client: &SupervisedClient, id: &str) -> bool {
    matches!(
        call(client, "files.choose.cancel", json!({"request":id})).await,
        Some(PortReply::Ok { .. })
    )
}

async fn recover_sending(
    client: &SupervisedClient,
    provider: &mut Provider,
    correlation: &str,
    id: Option<&str>,
) -> bool {
    let mut attempts = 0;
    loop {
        match provider.snapshot(client).await {
            Snapshot::Lost => return true,
            Snapshot::Live(value) => {
                let mut closed = true;
                for request in value["requests"]
                    .as_array()
                    .expect("checked")
                    .iter()
                    .filter(|r| {
                        id.map_or_else(
                            || r["correlation"].as_str() == Some(correlation),
                            |id| r["request"].as_str() == Some(id),
                        )
                    })
                {
                    if let Some(id) = request["request"].as_str() {
                        closed &= cancel_request(client, id).await;
                    } else {
                        closed = false;
                    }
                }
                if closed {
                    return true;
                }
                attempts += 1;
                // A live provider that refuses Close cannot safely hand off.
                // Report a final error instead of retrying forever or opening
                // a second window over the first.
                if attempts >= 3 {
                    return false;
                }
            }
            Snapshot::Retry => {}
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
