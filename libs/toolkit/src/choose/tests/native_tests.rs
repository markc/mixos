// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use bus::native_client::SupervisedClient;
use std::sync::Arc;

async fn publish(client: &SupervisedClient, value: serde_json::Value) {
    let mut frame = bus::BusMessage::new();
    frame.set("command", "files.chosen");
    frame.body = value.to_string();
    let headers = BTreeMap::from([
        ("name".into(), "files.chosen".into()),
        ("retain".into(), "false".into()),
    ]);
    let (rc, body, _) = client
        .call_with_headers_raw("noded", "topic.publish", &headers, &frame.to_wire())
        .await
        .unwrap();
    assert_eq!(rc, 0, "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn absent_noded_and_unregistered_files_skip_bus_without_caching() {
    let (_cancel_tx, mut cancel) = watch::channel(false);
    let req = request("{}").unwrap();
    assert!(
        native::attempt_at(&req, &mut cancel, "ws://127.0.0.1:1/ws")
            .await
            .is_none()
    );
    let broker = test_broker::Broker::start();
    assert!(
        native::attempt_at(&req, &mut cancel, &broker.url)
            .await
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_noded_early_topic_by_id_and_result_recovery() {
    let broker = test_broker::Broker::start();
    let provider = Arc::new(
        SupervisedClient::connect_options("files", &broker.url)
            .with_commands(
                files::chooser::COMMANDS
                    .iter()
                    .map(|(name, query)| bus::CommandDescriptor::new(name, &[], "test", *query))
                    .collect(),
            )
            .connect()
            .await
            .unwrap(),
    );
    let mut incoming = provider.incoming().unwrap();
    let client = provider.clone();
    let task = tokio::spawn(async move {
        while let Some(command) = incoming.recv().await {
            let body: serde_json::Value = serde_json::from_str(&command.body).unwrap_or(json!({}));
            let id = "0123456789abcdef-1";
            let outcome = json!({"request":id,"correlation":"compose-3","response":0,"uris":["file:///tmp/%FF.md"]});
            let answer = match command.command.as_str() {
                "files.choose" => {
                    assert_eq!(body["correlation"], "compose-3");
                    if body["title"] == "Early" {
                        publish(
                            &client,
                            json!({"request":"wrong-1","correlation":"compose-3","response":1}),
                        )
                        .await;
                        publish(&client, outcome.clone()).await;
                    }
                    json!({"request":id,"topic":"files.chosen","correlation":"compose-3"})
                }
                "files.choose.result" => json!({"state":"done","outcome":outcome}),
                _ => json!({}),
            };
            client
                .respond(&command, 0, &answer.to_string())
                .await
                .unwrap();
        }
    });
    for title in ["Early", "Recovery"] {
        let req = request(&json!({"title":title,"correlation":"compose-3"}).to_string()).unwrap();
        let (_cancel_tx, mut cancel) = watch::channel(false);
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            native::attempt_at(&req, &mut cancel, &broker.url),
        )
        .await
        .unwrap();
        let Some(Outcome::Chosen { paths, .. }) = result else {
            panic!("Bus did not choose: {result:?}")
        };
        assert_eq!(path_bytes(&paths[0]), b"/tmp/\xff.md");
    }
    task.abort();
    provider.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_real_bus_intent_keeps_egui_frames_running_and_cancel_is_final() {
    let broker = test_broker::Broker::start();
    let provider = Arc::new(
        SupervisedClient::connect_options("files", &broker.url)
            .with_commands(
                files::chooser::COMMANDS
                    .iter()
                    .map(|(name, query)| bus::CommandDescriptor::new(name, &[], "test", *query))
                    .collect(),
            )
            .connect()
            .await
            .unwrap(),
    );
    let mut incoming = provider.incoming().unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let p = provider.clone();
    let task = tokio::spawn(async move {
        while let Some(command) = incoming.recv().await {
            let value = if command.command == "files.choose" {
                started_tx.send(()).unwrap();
                gate.notified().await;
                json!({"request":"0123456789abcdef-1"})
            } else {
                json!({"state":"done","outcome":{"request":"0123456789abcdef-1","response":1}})
            };
            p.respond(&command, 0, &value.to_string()).await.unwrap();
        }
    });
    let mut handle = choose(request("{}").unwrap());
    let url = broker.url.clone();
    handle.driver = Some(Box::new(move |request, cancel, emitter| {
        runtime().block_on(async {
            let mut bus_cancel = cancel.clone();
            cascade(
                native::attempt_at(&request, &mut bus_cancel, &url),
                async { panic!("Cancel opened tier 2") },
                cancel,
                &emitter,
            )
            .await;
        });
    }));
    let ctx = Context::default();
    assert!(frame(&ctx, ViewportId::ROOT, &mut handle).is_none());
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    for _ in 0..100 {
        assert!(frame(&ctx, ViewportId::ROOT, &mut handle).is_none());
    }
    assert_eq!(handle.tier(), Tier::Bus);
    release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(outcome) = frame(&ctx, ViewportId::ROOT, &mut handle) {
                break outcome;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result, Outcome::Cancelled);
    task.abort();
    provider.close().await;
}
