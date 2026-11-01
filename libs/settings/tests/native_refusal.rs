// SPDX-License-Identifier: MIT OR Apache-2.0
#![cfg(feature = "native")]

use futures_util::{SinkExt, StreamExt};
use settings::{Binding, consumer::Consumer, native};
use serde_json::json;
use std::time::Duration;

/// Exercise both production adapters against actual correlated rc=10 frames.
#[tokio::test]
async fn real_refusal_reaches_native_consumer_and_apply_refresh_detector() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let advertised = Binding { instance: "authority".into(), profile: "default".into() };
    let body = json!({"status":"wrong_target", "binding":advertised}).to_string();
    let expected = body.clone();
    let broker = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        while let Some(Ok(frame)) = ws.next().await {
            let tokio_tungstenite::tungstenite::Message::Text(text) = frame else { continue };
            let req = bus::parse(&text).unwrap();
            let command = req.get("command").unwrap_or("");
            let refused = matches!(command, "settings.get" | "settings.apply");
            let mut reply = bus::BusMessage::new()
                .with_header("type", "response")
                .with_header("from", "settingsd")
                .with_header("command", command)
                .with_header("id", req.get("id").unwrap())
                .with_header("rc", if refused { "10" } else { "0" });
            reply.body = if refused { body.clone() } else { "{}".into() };
            ws.send(tokio_tungstenite::tungstenite::Message::Text(reply.to_wire().into())).await.unwrap();
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        let client = native::Client::connect("fixture", &url).await.unwrap();
        let mut consumer = Consumer::for_app(Binding { instance: "fixture".into(), profile: "default".into() }, "ced").unwrap();
        let subscribe = consumer.connected(client.connection_generation()).unwrap();
        let result = native::execute_until(&client, &subscribe, tokio::time::Instant::now() + Duration::from_secs(1)).await;
        let read = consumer.complete(&subscribe, result).unwrap();
        let fault = native::execute_until(&client, &read, tokio::time::Instant::now() + Duration::from_secs(1)).await.unwrap_err();
        assert_eq!(fault.code, "wrong_target");
        assert_eq!(fault.message, expected);
        let next = consumer.complete(&read, Err(fault)).unwrap();
        assert_eq!(next.binding(), &advertised);
        let error = client.call("settingsd", "settings.apply", json!({})).await.unwrap_err();
        assert!(native::wrong_target_refusal(&error));
        client.shutdown().await;
    }).await.unwrap();
    broker.abort();
}
