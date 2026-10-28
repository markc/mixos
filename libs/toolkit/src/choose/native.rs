// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use bus::native_client::{BoundedIncomingEvent, SupervisedClient};
use bus::PortReply;
use serde_json::{Value, json};

pub(super) async fn attempt(request: &Request, cancel: &mut watch::Receiver<bool>) -> Option<Outcome> {
    attempt_at(request, cancel, &bus::client_helpers::resolve_noded_url()).await
}

async fn call(client: &SupervisedClient, command: &str, body: Value) -> Option<PortReply> {
    tokio::time::timeout(IO_BOUND, client.call_typed("files", command, body)).await.ok()?.ok()
}

pub(super) async fn attempt_at(request: &Request, cancel: &mut watch::Receiver<bool>, url: &str) -> Option<Outcome> {
    let name = format!("toolkit-choose-{}-{}", std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed));
    let client = SupervisedClient::connect_options(&name, url).bounded_incoming(64).start();
    let result = session(&client, &name, request, cancel).await;
    client.close().await;
    result
}

async fn session(client: &SupervisedClient, name: &str, request: &Request, cancel: &mut watch::Receiver<bool>) -> Option<Outcome> {
    let connected = tokio::time::timeout(IO_BOUND, async {
        while !client.is_connected() { tokio::time::sleep(Duration::from_millis(10)).await; }
    });
    tokio::select! { result = connected => { result.ok()?; }, _ = cancelled(cancel) => return None }
    let services = tokio::time::timeout(IO_BOUND, client.list_services()).await.ok()?.ok()?;
    if !services.iter().any(|s| s == "files") { return None; }
    tokio::time::timeout(IO_BOUND, client.subscribe_topic("files.chosen")).await.ok()?.ok()?;
    let mut incoming = client.incoming_bounded()?;
    if *cancel.borrow() { return None; }
    // Do not abandon an in-flight intent on an observation deadline. A late
    // acknowledgement still owns a window and must be cancelled before fallback.
    let reply = client.call_typed("files", "files.choose", request.body()).await;
    let id = match reply {
        Ok(PortReply::Ok { value, .. }) => value["request"].as_str().map(str::to_owned),
        Ok(PortReply::AppError { .. }) => return None, // refusal opened no window
        Err(_) => None,
    };
    let Some(id) = id else {
        // Recover an acknowledgement lost in transport. Our per-handle caller
        // identity disambiguates even two requests with the same correlation.
        recover_sending(client, name, cancel).await;
        return None;
    };
    let mut failures = 0;
    let mut incoming_live = true;
    let mut sweep = tokio::time::interval(Duration::from_secs(5));
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancelled(cancel) => {
                cancel_request(client, &id).await;
                return None;
            }
            event = incoming.recv(), if incoming_live => {
                match event {
                    Some(BoundedIncomingEvent::Command(event)) if event.topic() == Some("files.chosen") => {
                        if let Ok(value) = serde_json::from_str::<Value>(&event.body)
                            && value["request"].as_str() == Some(&id) {
                            return Some(decode_outcome(request, &value));
                        }
                    }
                    None => incoming_live = false,
                    _ => { /* The result query recovers overflow and missed publications. */ }
                }
            }
            _ = sweep.tick() => {
                match call(client, "files.choose.result", json!({"request":id})).await {
                    Some(PortReply::Ok { value, .. }) if value["state"] == "done" => return Some(decode_outcome(request, &value["outcome"])),
                    Some(PortReply::Ok { value, .. }) if value["state"] == "open" => failures = 0,
                    Some(PortReply::AppError { .. }) => {
                        // A provider can have restarted; Close is idempotent.
                        if cancel_request(client, &id).await { return None; }
                    }
                    _ => {
                        failures += 1;
                        if failures >= 2 && cancel_request(client, &id).await { return None; }
                        // No switch while the previous window's end is unknown.
                    }
                }
            }
        }
    }
}

async fn cancel_request(client: &SupervisedClient, id: &str) -> bool {
    for _ in 0..3 {
        if matches!(call(client, "files.choose.cancel", json!({"request":id})).await, Some(PortReply::Ok { .. })) { return true; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn recover_sending(client: &SupervisedClient, name: &str, cancel: &watch::Receiver<bool>) {
    let caller = format!("local:{name}");
    let mut abandoned_attempts = 0;
    loop {
        if let Some(PortReply::Ok { value, .. }) = call(client, "files.choose.list", json!({})).await
            && let Some(requests) = value["requests"].as_array() {
            let mut closed = true;
            for request in requests.iter().filter(|r| r["caller"].as_str() == Some(&caller)) {
                if let Some(id) = request["request"].as_str() { closed &= cancel_request(client, id).await; }
            }
            if closed { return; }
        }
        if *cancel.borrow() {
            abandoned_attempts += 1;
            if abandoned_attempts >= 3 { return; }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
