// SPDX-License-Identifier: MIT OR Apache-2.0
//! `on @.verb` end to end: one generic script served twice, as
//! `mix --serve bridge.mix --name foo` and `--name other`, against a real
//! embedded `noded` broker (test-broker), driven only over native ABP.
//!
//! - `foo` answers `foo.call` and `other` answers `other.call`; neither
//!   answers the other's verb or the literal placeholder `@.call`;
//! - HELP lists the resolved `foo.call` with its `desc` text, never `@.call`;
//! - after RELOAD, a verb the new script adds with `@.` answers under the
//!   same serve name (the candidate generation resolves the same way).
//!
//! Run: `cargo test -p mix-shell --test serve_name_prefix`
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
on @.call desc "Answer with the serve name"
  reply(0, json_encode({served_by: serve_name(), verb: $event.command}))
end
"#;

const SCRIPT_RELOADED: &str = r#"-- version: 0.2.0
on @.call desc "Answer with the serve name"
  reply(0, json_encode({served_by: serve_name(), verb: $event.command}))
end
on @.added
  reply(0, json_encode({served_by: serve_name(), verb: $event.command}))
end
"#;

fn help_names(help: &Value) -> Vec<String> {
    help.as_array()
        .expect("HELP is a list")
        .iter()
        .filter_map(|e| e["name"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn placeholder_verbs_answer_under_each_instances_serve_name() {
    let dir = Dir::new("names");
    let mut broker = Broker::start();
    let node_conf = dir.write("node.conf.mix", &node_conf_text(broker_tcp_port(&broker)));
    let script = dir.write("bridge.mix", SCRIPT);
    let _foo = Citizen::spawn(&dir, &node_conf, &script, "foo");
    let _other = Citizen::spawn(&dir, &node_conf, &script, "other");
    let c = connect(&broker).await;

    let foo = wait_answer(&c, "foo", "foo.call", &dir).await;
    assert_eq!(foo, json!({"served_by": "foo", "verb": "foo.call"}));
    let other = wait_answer(&c, "other", "other.call", &dir).await;
    assert_eq!(other, json!({"served_by": "other", "verb": "other.call"}));

    // Neither instance answers the other's verb or the literal placeholder.
    assert!(call(&c, "foo", "other.call").await.is_err());
    assert!(call(&c, "foo", "@.call").await.is_err());
    assert!(call(&c, "other", "foo.call").await.is_err());

    let help = call(&c, "foo", "HELP").await.expect("HELP");
    let names = help_names(&help);
    assert!(names.iter().any(|n| n == "foo.call"), "{names:?}");
    assert!(!names.iter().any(|n| n.starts_with('@')), "{names:?}");
    let entry = help
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "foo.call")
        .unwrap();
    assert_eq!(entry["description"], "Answer with the serve name");
    let other_help = help_names(&call(&c, "other", "HELP").await.expect("HELP"));
    assert!(
        other_help.iter().any(|n| n == "other.call"),
        "{other_help:?}"
    );
    assert!(
        !other_help.iter().any(|n| n == "foo.call"),
        "{other_help:?}"
    );

    // RELOAD: the candidate generation resolves `@` the same way.
    dir.write("bridge.mix", SCRIPT_RELOADED);
    let ok = call(&c, "foo", "RELOAD").await.expect("RELOAD rc:0");
    assert_eq!(ok["reloading"], true);
    let added = wait_answer(&c, "foo", "foo.added", &dir).await;
    assert_eq!(added, json!({"served_by": "foo", "verb": "foo.added"}));
    let foo = call(&c, "foo", "foo.call")
        .await
        .expect("foo.call after reload");
    assert_eq!(foo["served_by"], "foo");
    let names = help_names(&call(&c, "foo", "HELP").await.expect("HELP"));
    assert!(names.iter().any(|n| n == "foo.added"), "{names:?}");

    broker.stop();
}
