// SPDX-License-Identifier: MIT OR Apache-2.0
//! Published events reach a subscriber through a real broker, in order,
//! with the loss reported on the first one sent (citizen::outbox).
use ::bus::native_client::{BoundedIncomingEvent, SupervisedClient};
use serde_json::{Value, json};
use std::time::Duration;

const TOPIC: &str = "pubtest.command.executed";

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
    let event = |seq: u64| json!({"v": 1, "seq": seq, "id": "x", "origin": "menu", "dropped": 0});
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
