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

/// Drain until all `total` frames are accounted for: each one retained or
/// counted by an overflow marker. The native and supervisor pumps finish on
/// their own schedule, so this polls within a bound rather than sleeping.
async fn drain_until_accounted(
    rx: &mut bus::native_client::BoundedIncomingReceiver,
    total: u64,
) -> (Vec<bus::IncomingCommand>, u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut retained = Vec::new();
    let mut dropped = 0;
    loop {
        let (batch, marked) = drain(rx);
        retained.extend(batch);
        dropped += marked;
        if retained.len() as u64 + dropped == total || tokio::time::Instant::now() >= deadline {
            return (retained, dropped);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Ids of every `BUSY` reply the stub has received so far.
async fn busy_ids(stub: &Stub) -> Vec<String> {
    stub.state
        .lock()
        .await
        .responses
        .iter()
        .filter(|r| r.get("rc") == Some("10"))
        .map(|r| r.get("id").unwrap().to_string())
        .collect()
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
    // Not consumed during the flood: the lane fills and the rest overflow.
    let mut incoming = client.incoming_bounded().expect("bounded receiver");
    barrier(&client).await;

    // The refusals are written by the supervisor and the lane is drained by its
    // owner, and neither finishes with the echo. How many requests are accepted
    // and how many are refused both depend on timing, so drain and sample until
    // every request is either accepted or answered BUSY.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut accepted: Vec<bus::IncomingCommand> = Vec::new();
    let mut dropped = 0;
    let busy = loop {
        let (retained, marked) = drain(&mut incoming);
        accepted.extend(retained);
        dropped += marked;
        let busy = busy_ids(&stub).await;
        let accepted_ids: Vec<String> = accepted.iter().filter_map(|c| c.id.clone()).collect();
        let union: BTreeSet<String> = busy.iter().chain(&accepted_ids).cloned().collect();
        if (union.len() == 128 && dropped == busy.len() as u64)
            || tokio::time::Instant::now() >= deadline
        {
            break busy;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    let accepted_ids: Vec<String> = accepted.iter().filter_map(|c| c.id.clone()).collect();
    assert!(!busy.is_empty(), "the flood must overflow the RPC lane at least once");
    assert!(!accepted.is_empty(), "the lane must retain at least one request");
    assert_eq!(dropped, busy.len() as u64, "every BUSY matches one overflow marker");
    assert!(accepted.iter().all(|c| c.generation == 1));

    let busy_set: HashSet<&String> = busy.iter().collect();
    assert_eq!(busy_set.len(), busy.len(), "no duplicate BUSY ids");
    let accepted_set: HashSet<&String> = accepted_ids.iter().collect();
    assert_eq!(accepted_set.len(), accepted_ids.len(), "no duplicate accepted ids");
    assert!(busy_set.is_disjoint(&accepted_set), "accepted and refused are disjoint");
    let union: BTreeSet<String> = busy.iter().chain(&accepted_ids).cloned().collect();
    let expected: BTreeSet<String> = (0..128).map(|n| format!("req-{n}")).collect();
    assert_eq!(union, expected, "every original request is accounted for");

    {
        let state = stub.state.lock().await;
        let busy_replies: Vec<&BusMessage> = state
            .responses
            .iter()
            .filter(|r| r.get("rc") == Some("10"))
            .collect();
        for reply in &busy_replies {
            assert_eq!(reply.get("type"), Some("response"));
            assert_eq!(reply.get("from"), Some("game-svc"));
            assert_eq!(reply.get("to"), Some("caller"));
            assert_eq!(reply.get("command"), Some("game.move"));
            let body: serde_json::Value = serde_json::from_str(&reply.body).expect("JSON body");
            assert_eq!(body["error_code"], "BUSY");
            assert!(body["message"].is_string());
        }
    }

    // Outbound correlation still resolves with the lane drained. The stub reads
    // frames in order, so any BUSY written before this echo is already recorded
    // when it returns: the count must not grow after every request is accounted.
    barrier(&client).await;
    assert_eq!(busy_ids(&stub).await.len(), busy.len(), "no BUSY reply after the echo");
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

    let (retained, dropped) = drain_until_accounted(&mut incoming, 128).await;
    assert!(dropped > 0, "the burst must overflow capacity 2");
    assert_eq!(retained.len() as u64 + dropped, 128, "every frame is retained or counted as dropped");
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

    let (retained, dropped) = drain_until_accounted(&mut incoming, 128).await;
    assert!(dropped > 0, "the burst must overflow capacity 2");
    assert_eq!(retained.len() as u64 + dropped, 128, "every frame is retained or counted as dropped");
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
