// SPDX-License-Identifier: MIT OR Apache-2.0
//! The real CLI forwards to an existing citizen without starting eframe.
use futures::StreamExt;
use serde_json::json;
use std::time::Duration;

#[test]
fn compose_launches_forward_empty_and_document_requests() {
    let broker = test_broker::Broker::start();
    let (handle, mut deliveries) = citizen::start("email", "email-test", &broker.url).unwrap();
    let reply = handle.clone();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let mut bodies = Vec::new();
            tokio::time::timeout(Duration::from_secs(20), async {
                while bodies.len() < 2 {
                    if let Some(citizen::Delivery::Command { id, command, body }) = deliveries.next().await {
                        match command.as_str() {
                            "email.ping" => reply.reply(id, 0, json!({"pong":true})),
                            "email.compose" => {
                                bodies.push(serde_json::from_str::<serde_json::Value>(&body).unwrap());
                                reply.reply(id, 0, json!({"opened":true}));
                            }
                            other => panic!("unexpected forwarded command: {other}"),
                        }
                    }
                }
            }).await.unwrap();
            bodies
        })
    });
    let dir = tempfile::tempdir().unwrap();
    for file in [None, Some("message.md")] {
        let mut cli = std::process::Command::new(env!("CARGO_BIN_EXE_email"));
        cli.current_dir(dir.path()).arg("--compose");
        if let Some(file) = file { cli.arg(file); }
        let output = cli.args(["--noded-url", &broker.url, "--service", "email-test"])
            .env_remove("WAYLAND_DISPLAY").env_remove("DISPLAY")
            .env("XDG_CONFIG_HOME", dir.path()).env("XDG_STATE_HOME", dir.path())
            .output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let bodies = worker.join().unwrap();
    assert_eq!(bodies[0], json!({}));
    assert_eq!(bodies[1], json!({"document":dir.path().join("message.md")}));
    handle.quit();
    handle.wait_done().unwrap();
}
