// SPDX-License-Identifier: MIT OR Apache-2.0
//! The actual binary runs its core and Bus port without desktop access.
use bus::native_client::SupervisedClient;
use serde_json::json;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headless_binary_serves_local_tree_and_quits_without_a_display() {
    let broker = test_broker::Broker::start();
    let directory = tempfile::tempdir().unwrap();
    let client = SupervisedClient::connect_options("workshop-agent", &broker.url)
        .with_verbs(Vec::new())
        .connect()
        .await
        .unwrap();
    let mut app = Process(
        Command::new(env!("CARGO_BIN_EXE_workshop"))
            .args([
                "--headless",
                "--service",
                "workshop-test",
                "--noded-url",
                &broker.url,
            ])
            .env("HOME", directory.path())
            .env("XDG_STATE_HOME", directory.path())
            .env("MIXOS_APP_HOME", directory.path().join("app"))
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(tree) = client
                .call("workshop-test", "workshop.tree", json!({}))
                .await
                && tree["rows"].as_array().is_some_and(|rows| {
                    rows.iter()
                        .any(|row| row["key"] == "service:test-node:noded")
                })
            {
                break;
            }
            assert!(
                app.0.try_wait().unwrap().is_none(),
                "headless process exited early"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("local node and noded within 5 s");
    // The engine lane stays responsive while asynchronous descriptions run.
    for _ in 0..10 {
        tokio::time::timeout(
            Duration::from_millis(500),
            client.call("workshop-test", "workshop.info", json!({})),
        )
        .await
        .unwrap()
        .unwrap();
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    // The public quit verb is intentionally refused while discovery is busy.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let info = client
                .call("workshop-test", "workshop.info", json!({}))
                .await
                .unwrap();
            if info["discovering"] == false {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("discovery settles before quit");
    client
        .call("workshop-test", "workshop.quit", json!({}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while app.0.try_wait().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("quit drains the Bus and exits");
    client.close().await;
}
