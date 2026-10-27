// SPDX-License-Identifier: MIT OR Apache-2.0
//! Live diagnostics remain available when every UI request slot is occupied.
use bus::native_client::SupervisedClient;
use futures::StreamExt;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn debug_bypasses_ui_capacity_and_serves_instance_and_discovery_names() {
    let mut args = vec!["fixture".into()];
    let _logging = citizen::debug::init("fixture", "warn", &mut args).unwrap();
    let broker = test_broker::Broker::start();
    let (app, mut deliveries) = citizen::start("fixture", "fixture-instance", &broker.url).unwrap();
    let caller = Arc::new(SupervisedClient::connect("debug-caller", &broker.url).await.unwrap());
    let mut pending = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let caller = caller.clone();
        pending.spawn(async move {
            caller.call("fixture-instance", "fixture.wait", json!({})).await
        });
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut received = 0;
        while received < 32 {
            if let Some(citizen::Delivery::Command { .. }) = deliveries.next().await {
                received += 1;
            }
        }
    }).await.expect("all UI slots occupied");
    let initial = caller.call("fixture-instance", "fixture.debug", json!({})).await.unwrap();
    let enabled = caller.call("fixture-instance", "fixture-instance.debug.set", json!({"level":"trace"})).await.unwrap();
    assert_eq!(enabled["level"], "trace");
    let restored = caller.call("fixture-instance", "fixture.debug.set", json!({"on":false})).await.unwrap();
    assert_eq!(restored["filter"], initial["baseline"]);
    app.quit();
    app.wait_done().unwrap();
    while pending.join_next().await.is_some() {}
    caller.close().await;
}
