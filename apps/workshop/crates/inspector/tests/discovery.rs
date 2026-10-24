// SPDX-License-Identifier: MIT OR Apache-2.0
//! A fake broker deliberately never answers one node, while its socket
//! continues serving the healthy nodes through the real citizen transport.
use futures::{SinkExt, StreamExt};
use inspector::{Engine, bus as discovery};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_node_cannot_hide_local_or_healthy_nodes_and_refresh_retries() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let recovered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let recovery = recovered.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        while let Some(Ok(Message::Text(wire))) = socket.next().await {
            let request = ::bus::parse(&wire).unwrap();
            if request.get("type") == Some("response") { continue; }
            let target = request.get("to").unwrap_or_default();
            if target.contains("slow") && !recovery.load(std::sync::atomic::Ordering::SeqCst) { continue; }
            let body = match request.get("command").unwrap_or_default() {
                "noded.info" => json!({"name":"this"}),
                "noded.list" => json!([]),
                "noded.peers" => json!({"peers":[{"name":"slow"},{"name":"alpha"},{"name":"beta"},{"name":"gamma"}]}),
                "HELP" => json!([{"name":"noded.info","read_only":true}]),
                _ => json!({}),
            };
            let mut reply = ::bus::BusMessage::new()
                .with_header("type", "response")
                .with_header("from", target)
                .with_header("command", request.get("command").unwrap())
                .with_header("id", request.get("id").unwrap_or_default())
                .with_header("rc", "0");
            reply.body = body.to_string();
            socket.send(Message::Text(reply.to_wire().into())).await.unwrap();
        }
    });
    let (handle, _events) = discovery::start("discovery-test", &url, &[]).unwrap();
    let mut engine = Engine::new(str::to_owned);
    let ticket = match engine.take_effects().pop().unwrap() {
        inspector::Effect::Discover { ticket } => ticket,
        other => panic!("{other:?}"),
    };
    let started = Instant::now();
    let mut healthy_seen = false;
    let mut updates = 0;
    let snapshot = tokio::time::timeout(Duration::from_secs(5), discovery::discover_incremental(handle.clone(), |snapshot| {
        updates += 1;
        if !healthy_seen && snapshot.nodes.iter().filter(|node| node.error.is_none()).count() == 4 {
            healthy_seen = true;
            assert!(started.elapsed() < discovery::DISCOVERY_CALL_BUDGET);
            assert!(snapshot.nodes.iter().any(|node| node.local && node.name == "this"));
            assert!(snapshot.services.contains_key("noded"));
        }
        engine.discovered(ticket, snapshot);
    })).await.expect("one slow node must cost one deadline, not serial info/list deadlines");
    assert!(healthy_seen && updates >= 6);
    assert!(started.elapsed() >= discovery::DISCOVERY_CALL_BUDGET);
    assert!(snapshot.progress.unwrap().finished);
    assert_eq!(snapshot.nodes.len(), 5);
    assert!(snapshot.nodes.iter().find(|node| node.name == "slow").unwrap().error.as_ref().unwrap().contains("deadline"));
    assert!(engine.node_label("slow").contains("unreachable"));
    let info: Value = engine.info();
    assert_eq!(info["discovering"], false);
    assert!(info["status"].as_str().unwrap().contains("5 of 5 nodes, 1 unreachable"));
    recovered.store(true, std::sync::atomic::Ordering::SeqCst);
    let refreshed = discovery::discover(handle.clone()).await;
    assert!(refreshed.nodes.iter().all(|node| node.error.is_none()));
    handle.quit();
    // wait_done is deliberately a synchronous shutdown join, outside UI logic.
    handle.wait_done().unwrap();
    server.abort();
}
