// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Real CLI processes, real broker and stdin/stdout pipes. No model is loaded.
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Child(tokio::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("model");
    std::fs::create_dir(&model).unwrap();
    for name in ["config.json", "tokenizer.json", "model.safetensors"] {
        std::fs::write(model.join(name), "invalid fixture: never loaded").unwrap();
    }
    let path = dir.path().join("config.conf.mix");
    std::fs::write(
        &path,
        json!({"service":{"vectors_db":dir.path().join("vectors.db"),"model_dir":model}})
            .to_string(),
    )
    .unwrap();
    (dir, path)
}

async fn ready(url: &str) {
    for _ in 0..100 {
        if let Ok(client) = ::bus::native_client::NodedClient::connect_anonymous(url).await {
            let reply = client.call("indexd", "indexd.stats", json!({})).await;
            client.close().await;
            if reply.is_ok() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("indexd did not register/reconnect");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_reconnect_bridge_flush_eof_and_sigterm() {
    let mut broker = test_broker::Broker::start();
    let (_dir, config) = fixture();
    let node_config = _dir.path().join("node.conf.mix");
    let port = broker
        .url
        .strip_prefix("ws://127.0.0.1:")
        .unwrap()
        .strip_suffix("/ws")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    std::fs::write(
        &node_config,
        json!({"wg_ip":"127.0.0.1","noded":{"port":port}}).to_string(),
    )
    .unwrap();
    let mut daemon = Child(
        tokio::process::Command::new(env!("CARGO_BIN_EXE_indexd"))
            .args(["-c", config.to_str().unwrap(), "serve"])
            .env_remove("MIXOS_NODED_URL")
            .env("MIXOS_NODE_CONFIG", &node_config)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    ready(&broker.url).await;
    broker.bounce();
    // Broker::start uses an ephemeral port; refresh config before reconnect.
    let port = broker
        .url
        .strip_prefix("ws://127.0.0.1:")
        .unwrap()
        .strip_suffix("/ws")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    std::fs::write(
        &node_config,
        json!({"wg_ip":"127.0.0.1","noded":{"port":port}}).to_string(),
    )
    .unwrap();
    ready(&broker.url).await;
    let mut bridge = Child(
        tokio::process::Command::new(env!("CARGO_BIN_EXE_indexd"))
            .arg("bridge")
            .arg("--node-config")
            .arg(&node_config)
            .env_remove("MIXOS_NODED_URL")
            .env("MIXOS_NODE_CONFIG", "/nonexistent/overridden.conf.mix")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = bridge.0.stdin.take().unwrap();
    let mut lines = BufReader::new(bridge.0.stdout.take().unwrap()).lines();
    for invalid in [vec![0xff, b'\n'], vec![b'x'; ::bus::WS_MAX_FRAME_BYTES + 1]] {
        input.write_all(&invalid).await.unwrap();
        if invalid.last() != Some(&b'\n') {
            input.write_all(b"\n").await.unwrap();
        }
        input.flush().await.unwrap();
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({"id":null,"rc":-1,"error":"invalid bridge request"})
        );
    }
    for (id, command_name, body, rc) in [
        (
            json!({"correlation":[1,"x"]}),
            "indexd.stats",
            json!({"action":"stats"}),
            0,
        ),
        (
            json!(null),
            "indexd.store",
            json!({"texts":["x"],"metadata":[{}]}),
            10,
        ),
        (json!(3), "indexd.unknown", json!({}), 10),
    ] {
        input
            .write_all(format!("{}\n", json!({"id":id,"command":command_name,"body":body})).as_bytes())
            .await
            .unwrap();
        input.flush().await.unwrap();
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], id);
        assert_eq!(response["rc"], rc);
        if rc == 10 {
            assert!(response["body"]["error"].is_string());
        } else {
            assert_eq!(response["body"]["total_vectors"], 0);
        }
    }
    // Keep the bridge connection open while stopping the service and broker.
    // SAFETY: signal only this test's child process.
    assert_eq!(
        unsafe { libc::kill(daemon.0.id().unwrap() as i32, libc::SIGTERM) },
        0
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(5), daemon.0.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    broker.stop();
    input
        .write_all(b"{\"id\":4,\"command\":\"indexd.stats\",\"body\":{}}\n")
        .await
        .unwrap();
    let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["id"], 4);
    assert_eq!(response["rc"], -1);
    assert!(response["error"].is_string());
    assert!(
        !tokio::time::timeout(Duration::from_secs(5), bridge.0.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(lines.next_line().await.unwrap().is_none());
}

#[test]
fn bridge_initial_connection_failure_replies_then_exits_nonzero() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("node.conf.mix");
    std::fs::write(&path, "noded: { port: 0 }\n").unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_indexd"))
        .arg("bridge")
        .env_remove("MIXOS_NODED_URL")
        .env("MIXOS_NODE_CONFIG", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(b"{\"id\":7,\"command\":\"indexd.stats\",\"body\":{}}\n")
        .unwrap();
    // Keep stdin open: exit must not depend on EOF.
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["id"], 7);
    assert_eq!(reply["rc"], -1);
}

#[tokio::test]
async fn bridge_honours_url_override_with_empty_node_config() {
    use tokio::io::AsyncWriteExt;
    let broker = test_broker::Broker::start();
    let mut child = Child(
        tokio::process::Command::new(env!("CARGO_BIN_EXE_indexd"))
            .arg("bridge")
            .env("MIXOS_NODE_CONFIG", "")
            .env("MIXOS_NODED_URL", &broker.url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    input
        .write_all(b"{\"id\":1,\"command\":\"indexd.stats\",\"body\":{}}\n")
        .await
        .unwrap();
    drop(input);
    let mut lines = BufReader::new(child.0.stdout.take().unwrap()).lines();
    let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["rc"], 10);
    assert!(reply["body"].is_null());
    assert!(reply["error"].as_str().unwrap().contains("not found"));
    assert!(child.0.wait().await.unwrap().success());
}

#[test]
fn call_is_in_process_and_uses_explicit_config() {
    let (_dir, config) = fixture();
    let run = |command_name: &str, body: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_indexd"))
            .args(["--config", config.to_str().unwrap(), "call", command_name, body])
            .env("MIXOS_NODED_URL", "ws://127.0.0.1:1/ws")
            .output()
            .unwrap()
    };
    let stats = run("indexd.stats", "{}");
    assert!(stats.status.success());
    let reply: Value = serde_json::from_slice(&stats.stdout).unwrap();
    assert_eq!(reply["total_vectors"], 0);
    let bad = run("indexd.nope", "{}");
    assert_eq!(bad.status.code(), Some(1));
    assert!(serde_json::from_slice::<Value>(&bad.stdout).unwrap()["error"].is_string());
}

#[test]
fn call_charges_original_argv_bytes_including_whitespace() {
    let (_dir, config) = fixture();
    let mut value: Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    value["service"]["max_request_line_bytes"] = json!(128);
    std::fs::write(&config, value.to_string()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_indexd"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "call",
            "stats",
            &format!("{}{{}}", " ".repeat(128)),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["code"],
        "request_too_large"
    );
}

#[test]
fn call_requires_offline_for_writes_and_always_refuses_background() {
    let (_dir, config) = fixture();
    let run = |command_name: &str, body: &str, offline: bool| {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_indexd"));
        command.args(["-c", config.to_str().unwrap(), "call", command_name, body]);
        if offline {
            command.arg("--offline");
        }
        command.output().unwrap()
    };
    for action in [
        "store",
        "update",
        "delete",
        "feedback",
        "supersede",
        "index_file",
    ] {
        for command_name in [action.to_string(), format!("indexd.{action}")] {
            let output = run(&command_name, "{}", false);
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("would desynchronise"));
            let output = run(&command_name, "{}", true);
            assert!(!String::from_utf8_lossy(&output.stderr).contains("require --offline"));
        }
    }
    for command_name in ["search", "list", "stats", "stale", "embed"] {
        let output = run(command_name, "{}", false);
        assert!(!String::from_utf8_lossy(&output.stderr).contains("require --offline"));
        assert!(serde_json::from_slice::<Value>(&output.stdout).is_ok());
    }
    assert!(!run("delete", r#"{"ids":[]}"#, true).status.success());
    for offline in [false, true] {
        for command_name in ["indexd.index_file", "stats"] {
            let output = run(command_name, r#"{"background":true}"#, offline);
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("no process remains"));
        }
    }
}

#[test]
fn desktop_package_carries_indexd_and_owned_units() {
    let mix = std::env::var("MIXOS_TEST_MIX").unwrap_or_else(|_| "/opt/mixos/bin/mix".into());
    let output = std::process::Command::new(mix)
        .args([
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/package_test.mix"),
            env!("CARGO_BIN_EXE_indexd"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
}
