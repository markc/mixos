// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Broadcast condition acceptance over an owned private broker.
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

/// Test-owned temp dir: node.conf, the script, citizen stderr.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!(
            "mix-serve-prefix-{tag}-{}-{nanos:x}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, contents).unwrap();
        p
    }
    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.0.join(name)).unwrap_or_default()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A `mix --serve` subprocess in its own process group; Drop SIGKILLs it.
struct Citizen {
    pgid: libc::pid_t,
    child: Child,
}

impl Citizen {
    fn spawn(dir: &Dir, node_conf: &Path, script: &Path, name: &str) -> Citizen {
        let stderr = File::create(dir.0.join(format!("{name}.stderr"))).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mix"));
        cmd.args([
            "--no-prelude",
            "--serve",
            script.to_str().unwrap(),
            "--name",
            name,
        ])
        .env("HOME", &dir.0)
        .env("MIXOS_NODE_CONFIG", node_conf)
        .env("MIXOS_ETC", &dir.0)
        .env("MIX_STATS", "off")
        .env_remove("MIXOS_SESSION_FD")
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
}

impl Drop for Citizen {
    fn drop(&mut self) {
        unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

fn node_conf_text(port: u16) -> String {
    format!("wg_ip: \"127.0.0.1\"\nnoded: {{ port: {port} }}\n")
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

async fn call(c: &VerifiedConnection, svc: &str, cmd: &str) -> Result<Value, String> {
    c.client()
        .call(svc, cmd, json!({}))
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Poll until `svc` answers `cmd`, returning the reply.
async fn wait_answer(c: &VerifiedConnection, svc: &str, cmd: &str, dir: &Dir) -> Value {
    let deadline = Instant::now() + HARD;
    loop {
        match call(c, svc, cmd).await {
            Ok(v) => return v,
            Err(e) => assert!(
                Instant::now() < deadline,
                "{svc} never answered {cmd} within {HARD:?}: {e}\nstderr:\n{}",
                dir.read(&format!("{svc}.stderr"))
            ),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

const SCRIPT: &str = r#"-- version: 0.1.0
$state = {rev: 0, waiting: 0}
on @.ping
  reply(0, json_encode($state))
end
on @.wait async
  $state.waiting = $state.waiting + 1
  while $state.rev == 0
    if not cond_wait("rev", 5) then break end
  end
  reply(0, json_encode({rev: $state.rev}))
end
on @.notify
  $state.rev = 1
  reply(0, json_encode({woke: cond_notify("rev")}))
end
"#;

#[tokio::test]
async fn waits_hold_open_while_pings_interleave_then_broadcast_releases_all() {
    let dir = Dir::new("conditions");
    let mut broker = Broker::start();
    let node_conf = dir.write("node.conf.mix", &node_conf_text(broker_tcp_port(&broker)));
    let script = dir.write("conditions.mix", SCRIPT);
    let _citizen = Citizen::spawn(&dir, &node_conf, &script, "conditions");
    let c = connect(&broker).await;
    wait_answer(&c, "conditions", "conditions.ping", &dir).await;
    let a = connect(&broker).await;
    let b = connect(&broker).await;
    let a = tokio::spawn(async move { call(&a, "conditions", "conditions.wait").await });
    let b = tokio::spawn(async move { call(&b, "conditions", "conditions.wait").await });
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let ping = call(&c, "conditions", "conditions.ping").await.unwrap();
            if ping["waiting"] == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!a.is_finished() && !b.is_finished());
        for _ in 0..5 {
            assert_eq!(
                call(&c, "conditions", "conditions.ping").await.unwrap()["rev"],
                0
            );
        }
        assert_eq!(
            call(&c, "conditions", "conditions.notify").await.unwrap()["woke"],
            2
        );
        assert_eq!(a.await.unwrap().unwrap()["rev"], 1);
        assert_eq!(b.await.unwrap().unwrap()["rev"], 1);
    })
    .await
    .expect("ping and notify cannot be held behind wait requests");
    broker.stop();
}
