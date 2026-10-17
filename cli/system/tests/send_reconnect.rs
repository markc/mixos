// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A failed Bus connect is no longer sticky for `send` (TODO-mix
//! 2026-10-10).
//!
//! One failed connect used to latch the in-process client "unavailable":
//! every later `send` answered `-3` until the script called
//! `bus_reconnect()`, even after `MIXOS_NODED_URL` pointed at a live
//! broker, and a script that outlived a broker restart stayed dead. A
//! `send` now re-probes (bounded by its `timeout=`) and re-reads the URL.
//!
//! The broker is the real embedded noded (`test-broker`). The verified
//! Unix lane is disabled with a missing `MIXOS_NODE_CONFIG`, so the probe
//! takes the URL path these tests steer.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use test_broker::Broker;

/// One broker fixture at a time: the bounce test reuses a fixed port.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn mix(dir: &Path, script: &str) -> Command {
    let path = dir.join("probe.mix");
    std::fs::write(&path, script).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mix"));
    cmd.arg("--no-prelude")
        .arg(&path)
        .env("HOME", dir)
        .env("MIX_STATS", "off")
        .env("MIXOS_NODE_CONFIG", dir.join("no-such-node.conf.mix"))
        .env_remove("MIXOS_NODED_URL")
        .env_remove("COSMIX_SESSION_FD")
        .stdin(Stdio::null());
    cmd
}

fn closed_url() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    format!("ws://127.0.0.1:{port}/ws")
}

/// `rc=` values the script printed, in order.
fn rcs(stdout: &str) -> Vec<i64> {
    stdout
        .lines()
        .filter_map(|l| l.strip_prefix("rc="))
        .map(|v| v.trim().parse().unwrap())
        .collect()
}

#[test]
fn a_send_after_a_failed_connect_follows_a_new_url() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let broker = Broker::start();
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        "export MIXOS_NODED_URL=\"{dead}\"\n\
         send zz-no-such-svc ping timeout=2\n\
         print(\"rc=${{rc}}\")\n\
         print(\"result=${{result}}\")\n\
         export MIXOS_NODED_URL=\"{live}\"\n\
         send zz-no-such-svc ping timeout=2\n\
         print(\"rc=${{rc}}\")\n",
        dead = closed_url(),
        live = broker.url,
    );
    let out = mix(dir.path(), &script).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    let got = rcs(&stdout);
    assert_eq!(got.len(), 2, "{stdout}\n{stderr}");
    assert_eq!(got[0], -3, "no broker: rc -3\n{stdout}");
    assert!(
        stdout.contains("next send retries"),
        "the -3 message must say a retry happens: {stdout}"
    );
    // The broker answered (an unknown service is a broker refusal, rc >= 10)
    // instead of the latched -3.
    assert!(
        got[1] >= 0,
        "send stayed latched after the URL changed: {stdout}"
    );
}

#[test]
fn a_send_after_a_broker_restart_reconnects() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut broker = Broker::with_tcp_port(port);
    let dir = tempfile::tempdir().unwrap();
    let sent = dir.path().join("sent");
    let go = dir.path().join("go");
    let script = format!(
        "export MIXOS_NODED_URL=\"{url}\"\n\
         send zz-no-such-svc ping timeout=2\n\
         print(\"rc=${{rc}}\")\n\
         write_file({sent:?}, \"1\")\n\
         $n = 0\n\
         while not is_file({go:?}) and $n < 500\n\
         $n = $n + 1\n\
         sleep(0.02)\n\
         end\n\
         send zz-no-such-svc ping timeout=2\n\
         print(\"rc=${{rc}}\")\n\
         send zz-no-such-svc ping timeout=2\n\
         print(\"rc=${{rc}}\")\n",
        url = broker.url,
        sent = sent.to_str().unwrap(),
        go = go.to_str().unwrap(),
    );
    let child = mix(dir.path(), &script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !sent.is_file() {
        assert!(Instant::now() < deadline, "the first send never finished");
        std::thread::sleep(Duration::from_millis(10));
    }
    broker.bounce();
    std::fs::write(&go, "1").unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    let got = rcs(&stdout);
    assert_eq!(got.len(), 3, "{stdout}\n{stderr}");
    assert!(got[0] >= 0, "first send must reach the broker: {stdout}");
    // The send that meets the dead connection may fail (-1); the one after
    // it must have reconnected rather than stay latched.
    assert!(
        got[2] >= 0,
        "send never reconnected after the broker restarted: {stdout}\n{stderr}"
    );
}
