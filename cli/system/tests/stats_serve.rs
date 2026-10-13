// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Usage statistics from a real `mix --serve` citizen against an embedded
//! `noded` broker (test-broker): each generation's counters land once, on
//! RELOAD (the retiring generation) and on SIGTERM (the live one).
//!
//! Run: `cargo test -p system --test stats_serve`
#![cfg(target_os = "linux")]

use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ::bus::native_client::{NodedClient, UnixConnectOutcome, VerifiedConnection};
use serde_json::{Value, json};
use test_broker::Broker;

const HARD: Duration = Duration::from_secs(30);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("mix-stats-serve-{}-{nanos:x}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A `mix --serve` citizen with statistics on, in its own process group.
struct Citizen {
    pgid: libc::pid_t,
    child: Child,
}

impl Citizen {
    fn spawn(dir: &Path, node_conf: &Path, script: &Path) -> Citizen {
        let stderr = File::create(dir.join("citizen.stderr")).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mix"));
        cmd.args([
            "--no-prelude",
            "--serve",
            script.to_str().unwrap(),
            "--name",
            "counter",
        ])
        .env("HOME", dir)
        .env("XDG_STATE_HOME", dir)
        .env("MIXOS_NODE_CONFIG", node_conf)
        .env("MIXOS_ETC", dir)
        .env_remove("MIX_STATS")
        .env_remove("COSMIX_SESSION_FD")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().unwrap();
        Citizen {
            pgid: child.id() as libc::pid_t,
            child,
        }
    }

    /// SIGTERM, which takes the normal flush path, then wait for exit.
    fn terminate(&mut self) {
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + HARD;
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "citizen ignored SIGTERM");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Citizen {
    fn drop(&mut self) {
        unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

fn broker_tcp_port(broker: &Broker) -> u16 {
    broker
        .url
        .trim_start_matches("ws://127.0.0.1:")
        .trim_end_matches("/ws")
        .parse()
        .unwrap()
}

async fn connect(broker: &Broker) -> VerifiedConnection {
    let UnixConnectOutcome::VerifiedUnix(c) =
        NodedClient::connect_unix("", &broker.url, &broker.options(), None)
            .await
            .expect("native control connect")
    else {
        panic!("control must be a verified Unix connection");
    };
    c
}

async fn call(c: &VerifiedConnection, cmd: &str) -> Result<Value, String> {
    c.client()
        .call("counter", cmd, json!({}))
        .await
        .map_err(|e| format!("{e:#}"))
}

async fn wait_answer(c: &VerifiedConnection, cmd: &str, dir: &Path) -> Value {
    let deadline = Instant::now() + HARD;
    loop {
        match call(c, cmd).await {
            Ok(value) => return value,
            Err(error) => assert!(
                Instant::now() < deadline,
                "counter never answered {cmd}: {error}\nstderr:\n{}",
                std::fs::read_to_string(dir.join("citizen.stderr")).unwrap_or_default()
            ),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

const SCRIPT: &str = r#"-- version: 0.1.0
on @.count
  reply(0, json_encode({n: length([1, 2])}))
end
"#;

#[tokio::test]
async fn serve_generations_each_record_their_counters_once() {
    let dir = Dir::new();
    let mut broker = Broker::start();
    let node_conf = dir.0.join("node.conf.mix");
    std::fs::write(
        &node_conf,
        format!(
            "wg_ip: \"127.0.0.1\"\nnoded: {{ port: {} }}\n",
            broker_tcp_port(&broker)
        ),
    )
    .unwrap();
    let script = dir.0.join("counter.mix");
    std::fs::write(&script, SCRIPT).unwrap();
    let mut citizen = Citizen::spawn(&dir.0, &node_conf, &script);
    let c = connect(&broker).await;

    wait_answer(&c, "counter.count", &dir.0).await;
    call(&c, "counter.count").await.expect("second call");
    let reloading = call(&c, "RELOAD").await.expect("RELOAD rc:0");
    assert_eq!(reloading["reloading"], true);
    // The new generation answers once its swap has committed.
    tokio::time::sleep(Duration::from_millis(500)).await;
    wait_answer(&c, "counter.count", &dir.0).await;
    citizen.terminate();
    broker.stop();

    let output = Command::new(env!("CARGO_BIN_EXE_mix"))
        .env("XDG_STATE_HOME", &dir.0)
        .env("HOME", &dir.0)
        .env_remove("MIX_STATS")
        .args(["stats", "raw"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let doc: Value = serde_json::from_str(stdout.split_once('\n').unwrap().1).unwrap();
    let serve_length: u64 = doc["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|bucket| bucket["mode"] == "serve")
        .filter_map(|bucket| bucket["builtins"]["length"].as_u64())
        .sum();
    assert_eq!(serve_length, 3, "three calls, each recorded once\n{stdout}");
    let serve_runs = doc["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|run| run["mode"] == "serve")
        .count();
    assert_eq!(serve_runs, 2, "one run per generation\n{stdout}");
}
