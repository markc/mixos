// SPDX-License-Identifier: MIT OR Apache-2.0
//! A fake broker deliberately never answers one node, while its socket
//! continues serving the healthy nodes through the real citizen transport.
use futures::{SinkExt, StreamExt};
use inspector::{Engine, bus as discovery};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mesh_latency_queue_and_failure_diagnostics() {
    use std::sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let failure = Arc::new(AtomicU8::new(0));
    let mode = failure.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let (mut writer, mut reader) = socket.split();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        loop {
            tokio::select! {
                Some(message) = rx.recv() => { if writer.send(message).await.is_err() { break; } }
                message = reader.next() => {
                    let Some(Ok(Message::Text(wire))) = message else { break; };
                    let request = ::bus::parse(&wire).unwrap();
                    if request.get("type") == Some("response") { continue; }
                    let target = request.get("to").unwrap_or_default();
                    let command = request.get("command").unwrap_or_default();
                    let failed = (mode.load(Ordering::SeqCst) == 1 && command == "noded.peers")
                        || (mode.load(Ordering::SeqCst) == 2 && target == "noded" && command == "noded.list");
                    let body = match command {
                        "noded.info" => json!({"name":"this"}),
                        "noded.list" => json!((0..if mode.load(Ordering::SeqCst) == 3 { 1000 } else { 12 }).map(|i| json!({"name":format!("svc-{i}")})).collect::<Vec<_>>()),
                        "noded.peers" if mode.load(Ordering::SeqCst) == 3 => json!({"peers":[]}),
                        "noded.peers" => json!({"peers":(0..8).map(|i| json!({"name":format!("peer-{i}")})).collect::<Vec<_>>()}),
                        "HELP" => json!([{"name":"ping","read_only":true}]),
                        _ => json!({}),
                    };
                    let mut reply = ::bus::BusMessage::new()
                        .with_header("type", "response").with_header("from", target)
                        .with_header("command", command)
                        .with_header("id", request.get("id").unwrap_or_default())
                        .with_header("rc", if failed { "10" } else { "0" });
                    reply.body = if failed { "forced discovery failure".into() } else { body.to_string() };
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        let _ = tx.send(Message::Text(reply.to_wire().into()));
                    });
                }
            }
        }
    });
    let (handle, _events) = discovery::start("latency-test", &url, &[]).unwrap();
    let snapshot =
        tokio::time::timeout(Duration::from_secs(15), discovery::discover(handle.clone()))
            .await
            .unwrap();
    assert_eq!(snapshot.nodes.len(), 9);
    assert_eq!(snapshot.services.len(), 9 * 13);
    assert!(
        snapshot.services.values().all(Result::is_ok),
        "{:?}",
        snapshot.services
    );
    failure.store(1, Ordering::SeqCst);
    let peers_failed = discovery::discover(handle.clone()).await;
    assert!(peers_failed.peer_error.is_some());
    let mut engine = Engine::new(str::to_owned);
    let inspector::Effect::Discover { ticket } = engine.take_effects().pop().unwrap() else {
        panic!()
    };
    engine.discovered(ticket, peers_failed);
    assert!(
        engine.rows().iter().any(|row| row["key"] == "mesh:error"
            && row["error"].as_str().unwrap().contains("noded.peers"))
    );
    assert!(engine.status.contains("noded.peers"));
    engine.snapshot = snapshot.clone();
    failure.store(2, Ordering::SeqCst);
    engine.command(99, "workshop.refresh", "{}");
    let ticket = engine
        .take_effects()
        .into_iter()
        .find_map(|e| match e {
            inspector::Effect::Discover { ticket } => Some(ticket),
            _ => None,
        })
        .unwrap();
    discovery::discover_incremental(handle.clone(), |part| engine.discovered(ticket, part)).await;
    assert!(engine.snapshot.error.is_some());
    assert_eq!(engine.snapshot.services, snapshot.services);
    assert!(
        engine
            .take_effects()
            .iter()
            .any(|effect| matches!(effect, inspector::Effect::Reply { id: 99, rc: 10, .. }))
    );
    assert!(!engine.busy());
    failure.store(3, Ordering::SeqCst);
    let large = tokio::time::timeout(Duration::from_secs(15), discovery::discover(handle.clone()))
        .await
        .unwrap();
    assert!(large.progress.unwrap().finished);
    assert_eq!(large.services.len(), 1001);
    assert_eq!(
        large
            .services
            .values()
            .filter(|result| result.is_ok())
            .count(),
        discovery::MAX_DESCRIBED_SERVICES_PER_NODE
    );
    assert_eq!(
        large
            .services
            .values()
            .filter(|result| matches!(result, Err(error) if error == "not described (limit)"))
            .count(),
        1001 - discovery::MAX_DESCRIBED_SERVICES_PER_NODE
    );
    handle.quit();
    handle.wait_done().unwrap();
    server.abort();
}

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
            if request.get("type") == Some("response") {
                continue;
            }
            let target = request.get("to").unwrap_or_default();
            if (target.contains("slow") || target == "service-0" || target == "service-1")
                && !recovery.load(std::sync::atomic::Ordering::SeqCst)
            {
                continue;
            }
            let body = match request.get("command").unwrap_or_default() {
                "noded.info" => json!({"name":"this"}),
                "noded.list" => json!(
                    (0..12)
                        .map(|i| json!({"name":format!("service-{i}")}))
                        .collect::<Vec<_>>()
                ),
                "noded.peers" => {
                    json!({"peers":[{"name":"slow"},{"name":"alpha"},{"name":"beta"},{"name":"gamma"}]})
                }
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
            socket
                .send(Message::Text(reply.to_wire().into()))
                .await
                .unwrap();
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
    let snapshot = tokio::time::timeout(
        Duration::from_secs(5),
        discovery::discover_incremental(handle.clone(), |snapshot| {
            updates += 1;
            if !healthy_seen
                && snapshot
                    .nodes
                    .iter()
                    .filter(|node| node.error.is_none())
                    .count()
                    == 4
            {
                healthy_seen = true;
                assert!(started.elapsed() < discovery::DISCOVERY_CALL_BUDGET);
                assert!(
                    snapshot
                        .nodes
                        .iter()
                        .any(|node| node.local && node.name == "this")
                );
                assert!(snapshot.services.contains_key("noded"));
            }
            engine.discovered(ticket, snapshot);
        }),
    )
    .await
    .expect("one slow node must cost one deadline, not serial info/list deadlines");
    assert!(healthy_seen && updates >= 6);
    assert!(started.elapsed() >= discovery::DISCOVERY_CALL_BUDGET);
    assert!(snapshot.progress.unwrap().finished);
    assert_eq!(snapshot.nodes.len(), 5);
    assert!(
        snapshot
            .services
            .iter()
            .filter(|(name, _)| name.as_str() != "service-0" && name.as_str() != "service-1")
            .all(|(_, result)| result.is_ok()),
        "healthy descriptions must not exhaust native call capacity: {:?}",
        snapshot.services
    );
    assert!(snapshot.services["service-0"].is_err());
    assert!(snapshot.services["service-1"].is_err());
    let slow_error = snapshot
        .nodes
        .iter()
        .find(|node| node.name == "slow")
        .unwrap()
        .error
        .as_ref()
        .unwrap();
    assert!(
        slow_error.contains("deadline") || slow_error.contains("timed out"),
        "{slow_error}"
    );
    assert!(engine.node_label("slow").contains("unreachable"));
    let info: Value = engine.info();
    assert_eq!(info["discovering"], false);
    assert!(
        info["status"]
            .as_str()
            .unwrap()
            .contains("5 of 5 nodes, 1 unreachable")
    );
    recovered.store(true, std::sync::atomic::Ordering::SeqCst);
    let refreshed = discovery::discover(handle.clone()).await;
    assert!(refreshed.nodes.iter().all(|node| node.error.is_none()));
    handle.quit();
    // wait_done is deliberately a synchronous shutdown join, outside UI logic.
    handle.wait_done().unwrap();
    server.abort();
}
