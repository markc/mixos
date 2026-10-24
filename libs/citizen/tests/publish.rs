// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Published events reach a subscriber through a real broker, in order,
//! with the loss reported on the first one sent (citizen::outbox).
use ::bus::native_client::{BoundedIncomingEvent, SupervisedClient};
use serde_json::{Value, json};
use std::time::Duration;

const TOPIC: &str = "pubtest.action.executed";

#[test]
fn pending_caller_receives_busy_when_citizen_closes() {
    use futures::StreamExt;
    let broker = test_broker::Broker::start();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (worker, mut deliveries) =
        citizen::start("closing-worker", "closing-worker", &broker.url).unwrap();
    let (caller, _) = citizen::start("closing-caller", "closing-caller", &broker.url).unwrap();
    runtime.block_on(async {
        let call = caller.call("closing-worker", "closing-worker.wait", json!({}));
        let close = async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(delivery) = deliveries.next().await {
                    if matches!(delivery, citizen::Delivery::Command { .. }) {
                        break;
                    }
                }
            })
            .await
            .unwrap();
            worker.quit();
        };
        let (reply, ()) = tokio::join!(call, close);
        let reply = reply.unwrap();
        assert_eq!(reply.rc, 10);
        assert_eq!(
            serde_json::from_str::<Value>(&reply.body).unwrap()["error_code"],
            "BUSY"
        );
    });
    worker.wait_done().unwrap();
    caller.quit();
    caller.wait_done().unwrap();
}

#[test]
fn runtime_subscription_delivers_bodies_and_unsubscribe_stops_delivery() {
    use futures::StreamExt;
    let broker = test_broker::Broker::start();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (watcher, mut deliveries) =
        citizen::start("runtime-watch", "runtime-watch", &broker.url).unwrap();
    let (publisher, _) = citizen::start("runtime-pub", "runtime-pub", &broker.url).unwrap();
    runtime.block_on(async {
        assert!(watcher.subscribe("").await.is_err());
        watcher.subscribe(TOPIC).await.unwrap();
        publisher.publish(TOPIC, vec![json!({"seq":1})], 0);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if matches!(deliveries.next().await, Some(citizen::Delivery::Topic(topic)) if topic == TOPIC) { break; }
            }
        }).await.unwrap();
        let rows = watcher.take_topic_events();
        assert_eq!(rows.len(), 1);
        assert_eq!(serde_json::from_str::<Value>(rows[0]["body"].as_str().unwrap()).unwrap()["seq"], 1);
        watcher.unsubscribe(TOPIC).await.unwrap();
        publisher.publish(TOPIC, vec![json!({"seq":2})], 0);
        let next = tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                if matches!(deliveries.next().await, Some(citizen::Delivery::Topic(topic)) if topic == TOPIC) { break; }
            }
        }).await;
        assert!(next.is_err());
        assert!(watcher.take_topic_events().is_empty());
    });
    watcher.quit();
    publisher.quit();
    watcher.wait_done().unwrap();
    publisher.wait_done().unwrap();
}

#[test]
fn published_events_reach_a_subscriber_in_order() {
    let broker = test_broker::Broker::start();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // The subscriber first, so nothing is published before it listens.
    let (watcher, mut incoming) = runtime.block_on(async {
        let watcher = SupervisedClient::connect_options("watcher", &broker.url)
            .bounded_incoming(64)
            .connect()
            .await
            .unwrap();
        let incoming = watcher.incoming_bounded().unwrap();
        watcher.subscribe_topic(TOPIC).await.unwrap();
        (watcher, incoming)
    });

    let (handle, _deliveries) = citizen::start("pubtest", "pubtest", &broker.url).unwrap();
    assert_eq!(handle.service(), "pubtest");
    let event = |seq: u64| json!({"v": 2, "params": null, "result": null, "seq": seq, "id": "x", "origin": "menu", "dropped": 0});
    // Three were lost before these two.
    handle.publish(TOPIC, vec![event(4), event(5)], 3);

    let got: Vec<Value> = runtime.block_on(async {
        let mut got = Vec::new();
        while got.len() < 2 {
            let next = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
                .await
                .expect("an event within 5 s");
            match next {
                Some(BoundedIncomingEvent::Command(cmd)) if cmd.topic() == Some(TOPIC) => {
                    got.push(serde_json::from_str(&cmd.body).unwrap());
                }
                Some(_) => {}
                None => panic!("the subscription ended"),
            }
        }
        got
    });
    let seen: Vec<(u64, u64)> = got
        .iter()
        .map(|b| (b["seq"].as_u64().unwrap(), b["dropped"].as_u64().unwrap()))
        .collect();
    assert_eq!(seen, vec![(4, 3), (5, 0)]);

    handle.quit();
    handle.wait_done().unwrap();
    runtime.block_on(watcher.close());
}
