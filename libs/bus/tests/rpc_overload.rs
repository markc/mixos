// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Bounded incoming lanes under a burst, against an in-process stub broker.
//! The opt-in RPC lane answers the id-bearing, topic-less requests it cannot
//! retain with a correlated `BUSY`; the plain bounded lane stays lossy; topic
//! and idless frames stay lossy under both.

#![cfg(feature = "client")]

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Duration;

use bus::native_client::BoundedIncomingEvent;
use bus::{BusMessage, SupervisedClient};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

#[derive(Default)]
struct StubState {
    /// Every `rc`-carrying reply the client sent back to the stub.
    responses: Vec<BusMessage>,
}

struct Stub {
    state: Mutex<StubState>,
    /// Frames pushed after `noded.register` is acknowledged, in order.
    burst: Vec<String>,
}

/// The stub's wire reply to `req`, correlated by command and id.
fn reply(req: &BusMessage, rc: &str) -> String {
    let mut m = BusMessage::new()
        .with_header("type", "response")
        .with_header("command", req.get("command").unwrap_or("?"))
        .with_header("from", "noded")
        .with_header("rc", rc);
    if let Some(id) = req.get("id") {
        m = m.with_header("id", id);
    }
    m.to_wire()
}

fn request(command: &str, from: &str, id: Option<&str>, topic: Option<&str>) -> String {
    let mut m = BusMessage::new()
        .with_header("type", "request")
        .with_header("command", command)
        .with_header("from", from);
    if let Some(id) = id {
        m = m.with_header("id", id);
    }
    if let Some(topic) = topic {
        m = m.with_header("topic", topic);
    }
    m.to_wire()
}

async fn run_stub(listener: TcpListener, stub: Arc<Stub>) {
    let Ok((tcp, _)) = listener.accept().await else {
        return;
    };
    let ws = tokio_tungstenite::accept_async(tcp).await.expect("handshake");
    let (mut sink, mut stream) = ws.split();
    while let Some(Ok(Message::Text(text))) = stream.next().await {
        let Ok(req) = bus::parse(&text) else { continue };
        if req.get("rc").is_some() {
            stub.state.lock().await.responses.push(req);
            continue;
        }
        match req.get("command").unwrap_or("") {
            "noded.register" => {
                let _ = sink.send(Message::Text(reply(&req, "0").into())).await;
                // The burst arrives strictly after the register acknowledgement.
                for frame in &stub.burst {
                    let _ = sink.send(Message::Text(frame.clone().into())).await;
                }
            }
            "echo.body" => {
                let mut response = bus::parse(&reply(&req, "0")).unwrap();
                response.body = req.body.clone();
                let _ = sink.send(Message::Text(response.to_wire().into())).await;
            }
            _ => {
                let _ = sink.send(Message::Text(reply(&req, "0").into())).await;
            }
        }
    }
}

async fn start(burst: Vec<String>) -> (String, Arc<Stub>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let stub = Arc::new(Stub {
        state: Mutex::new(StubState::default()),
        burst,
    });
    tokio::spawn(run_stub(listener, stub.clone()));
    (url, stub)
}

/// Poll until `cond` is true or the outer bound elapses.
async fn wait_until<F: Fn() -> bool>(secs: u64, cond: F) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cond()
}

/// Drain everything the bounded lane currently holds. Returns retained
/// commands and the total dropped count reported by overflow markers.
fn drain(
    rx: &mut bus::native_client::BoundedIncomingReceiver,
) -> (Vec<bus::IncomingCommand>, u64) {
    let mut commands = Vec::new();
    let mut dropped = 0;
    while let Some(event) = rx.try_recv() {
        match event {
            BoundedIncomingEvent::Command(c) => commands.push(c),
            BoundedIncomingEvent::Overflow { dropped: n } => dropped += n,
        }
    }
    (commands, dropped)
}

/// Round-trip one outbound echo. Replies are ordered behind the burst on the
/// one socket, so its return proves every burst frame was dispatched.
async fn barrier(client: &SupervisedClient) {
    let echoed = tokio::time::timeout(
        Duration::from_secs(5),
        client.call("peer", "echo.body", serde_json::json!("barrier")),
    )
    .await
    .expect("echo barrier within bound")
    .expect("echo barrier call");
    assert_eq!(echoed, serde_json::Value::String("barrier".into()));
}

fn game_move_burst(count: usize) -> Vec<String> {
    (0..count)
        .map(|n| request("game.move", "caller", Some(&format!("req-{n}")), None))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_lane_refuses_overflowed_requests_with_busy_and_loses_nothing() {
    let burst = game_move_burst(128);
    let (url, stub) = start(burst).await;
    let client = SupervisedClient::connect_options("game-svc", &url)
        .bounded_incoming_rpc(2)
        .connect()
        .await
        .expect("bounded rpc connect");
    // Not consumed during the flood: capacity 2 fills and the rest overflow.
    let mut incoming = client.incoming_bounded().expect("bounded receiver");
    barrier(&client).await;

    // Every overflowed request must be answered with BUSY: wait until the stub
    // has seen as many BUSY replies as the lane has overflowed.
    assert!(
        wait_until(5, || {
            let overflowed = incoming.overflow_count() as u64;
            stub.state
                .try_lock()
                .map(|s| {
                    let busy = s.responses.iter().filter(|r| r.get("rc") == Some("10")).count() as u64;
                    overflowed > 0 && busy == overflowed
                })
                .unwrap_or(false)
        })
        .await,
        "a BUSY reply expected for every overflowed request"
    );

    let (retained, dropped) = drain(&mut incoming);
    assert_eq!(retained.len(), 2, "capacity 2 retains exactly two");
    assert_eq!(dropped, incoming.overflow_count() as u64);
    assert_eq!(dropped + retained.len() as u64, 128, "every request is retained or refused");

    let accepted: Vec<String> = retained.iter().filter_map(|c| c.id.clone()).collect();
    assert!(retained.iter().all(|c| c.generation == 1));

    let state = stub.state.lock().await;
    let busy: Vec<&BusMessage> = state
        .responses
        .iter()
        .filter(|r| r.get("rc") == Some("10"))
        .collect();
    let busy_ids: Vec<String> = busy
        .iter()
        .map(|r| r.get("id").unwrap().to_string())
        .collect();
    for reply in &busy {
        assert_eq!(reply.get("type"), Some("response"));
        assert_eq!(reply.get("from"), Some("game-svc"));
        assert_eq!(reply.get("to"), Some("caller"));
        assert_eq!(reply.get("command"), Some("game.move"));
        let body: serde_json::Value = serde_json::from_str(&reply.body).expect("JSON body");
        assert_eq!(body["error_code"], "BUSY");
        assert!(body["message"].is_string());
    }

    let busy_set: HashSet<&String> = busy_ids.iter().collect();
    assert_eq!(busy_set.len(), busy_ids.len(), "no duplicate BUSY ids");
    let accepted_set: HashSet<&String> = accepted.iter().collect();
    assert!(busy_set.is_disjoint(&accepted_set), "accepted and refused are disjoint");
    let union: BTreeSet<String> = busy_ids.iter().chain(accepted.iter()).cloned().collect();
    let expected: BTreeSet<String> = (0..128).map(|n| format!("req-{n}")).collect();
    assert_eq!(union, expected, "every original request is accounted for");
    assert!(!busy_ids.is_empty() && !accepted.is_empty());
    drop(state);

    // Outbound correlation still resolves with the lane full, then after drain.
    barrier(&client).await;
    client.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_bounded_lane_stays_lossy_without_busy() {
    let (url, stub) = start(game_move_burst(128)).await;
    let client = SupervisedClient::connect_options("game-svc", &url)
        .bounded_incoming(2)
        .connect()
        .await
        .expect("bounded connect");
    let mut incoming = client.incoming_bounded().expect("bounded receiver");
    barrier(&client).await;

    let (retained, dropped) = drain(&mut incoming);
    assert!(dropped > 0, "the burst must overflow capacity 2");
    assert!(retained.len() <= 2);
    assert!(
        stub.state
            .lock()
            .await
            .responses
            .iter()
            .all(|r| r.get("rc") != Some("10")),
        "the default lane never answers with BUSY"
    );
    client.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn topic_and_idless_frames_stay_lossy_on_the_rpc_lane() {
    let mut burst = Vec::new();
    for n in 0..64 {
        burst.push(request("world.game.tick", "publisher", Some(&format!("t-{n}")), Some("world.game.tick")));
        burst.push(request("game.tick", "publisher", None, None));
    }
    let (url, stub) = start(burst).await;
    let client = SupervisedClient::connect_options("game-svc", &url)
        .bounded_incoming_rpc(2)
        .connect()
        .await
        .expect("bounded rpc connect");
    let mut incoming = client.incoming_bounded().expect("bounded receiver");
    barrier(&client).await;

    let (retained, dropped) = drain(&mut incoming);
    assert!(dropped > 0, "the burst must overflow capacity 2");
    assert!(retained.len() <= 2);
    for c in &retained {
        if c.headers.contains_key("topic") {
            assert_eq!(c.headers.get("topic").map(String::as_str), Some("world.game.tick"));
        } else {
            assert_eq!(c.id, None, "idless frames are the only other retained kind");
        }
    }
    assert!(
        stub.state
            .lock()
            .await
            .responses
            .iter()
            .all(|r| r.get("rc") != Some("10")),
        "topic and idless overflow must never produce BUSY"
    );
    client.close().await;
}
