// SPDX-License-Identifier: MIT OR Apache-2.0
//! Every personality of the system binary reports its own component and
//! version, never the host's: argv[0] picks the program, and build
//! provenance is captured inside each component's library. The `noded`
//! personality is also driven as a real process through a symlink, against
//! its own broker, and must stop cleanly on SIGTERM.

use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use bus::native_client::{BrokerAccount, NodedClient, UnixConnectOptions, UnixConnectOutcome};
use serde_json::json;

const BIN: &str = env!("CARGO_BIN_EXE_mix");

/// A fresh directory holding symlinks to the binary, one per name.
fn links(names: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for name in names {
        symlink(BIN, dir.path().join(name)).unwrap();
    }
    dir
}

fn run(command: &mut Command) -> (i32, String, String) {
    let out = command.output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn assert_shell_identity(command: &mut Command) {
    let (code, out, err) = run(command.args(["--version", "--json"]));
    assert_eq!(code, 0, "stderr: {err}");
    let doc: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(doc["component"], "mix");
    assert_eq!(doc["version"], mix_shell::COMPONENT_VERSION);
}

fn assert_noded_identity(command: &mut Command) {
    let (code, out, err) = run(command.args(["--version", "--json"]));
    assert_eq!(code, 0, "stderr: {err}");
    let doc: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(doc["component"], "noded");
    assert_eq!(doc["version"], noded::COMPONENT_VERSION);
    // The JSON form carries build provenance (buildinfo's --version --json
    // contract); the sha field must at least be present as a string.
    assert!(
        doc["git_sha"].as_str().is_some_and(|sha| !sha.is_empty()),
        "{doc}"
    );
}

#[test]
fn every_personality_reports_its_own_identity() {
    let dir = links(&["mix", "noded"]);

    let (code, out, err) = run(Command::new(dir.path().join("mix")).arg("--version"));
    assert_eq!(code, 0, "stderr: {err}");
    let want = format!("mix {} (", mix_shell::COMPONENT_VERSION);
    assert!(out.starts_with(&want), "got {out:?}, want prefix {want:?}");
    assert_shell_identity(&mut Command::new(dir.path().join("mix")));

    let (code, out, err) = run(Command::new(dir.path().join("noded")).arg("--version"));
    assert_eq!(code, 0, "stderr: {err}");
    let want = format!("noded {} (", noded::COMPONENT_VERSION);
    assert!(out.starts_with(&want), "got {out:?}, want prefix {want:?}");
    assert_noded_identity(&mut Command::new(dir.path().join("noded")));
}

#[test]
fn the_file_itself_is_the_shell() {
    assert_shell_identity(&mut Command::new(BIN));
}

#[test]
fn a_renamed_copy_is_the_shell() {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("development-copy");
    fs::copy(BIN, &copy).unwrap();
    assert_shell_identity(&mut Command::new(copy));
}

#[test]
fn a_login_shell_is_the_shell() {
    assert_shell_identity(Command::new(BIN).arg0("-mix"));
}

#[test]
fn an_unset_self_exec_name_is_the_shell() {
    assert_shell_identity(Command::new(BIN).arg0("/proc/self/exe"));
}

#[test]
fn personalities_lists_every_component() {
    let (code, out, _) = run(Command::new(BIN).arg("--personalities"));
    assert_eq!(code, 0);
    let want = format!(
        "mix {}\nnoded {}\n",
        mix_shell::COMPONENT_VERSION,
        noded::COMPONENT_VERSION
    );
    assert_eq!(out, want);
}

#[test]
fn an_unknown_name_exits_2_with_the_list() {
    let dir = links(&["maild"]);
    let (code, out, err) = run(&mut Command::new(dir.path().join("maild")));
    assert_eq!(code, 2);
    assert_eq!(out, "");
    assert!(err.starts_with("maild: not a personality"), "got {err:?}");
    assert!(
        err.contains(&format!("noded {}", noded::COMPONENT_VERSION)),
        "got {err:?}"
    );
}

/// The `noded` personality as a real process: the system binary started
/// through a symlink, in a throwaway root with its own config and log.
/// Drop kills a still-running child and removes the root, retaining
/// the evidence only when the test is already panicking.
struct NodedPersonality {
    root: PathBuf,
    child: Child,
}

impl Drop for NodedPersonality {
    fn drop(&mut self) {
        // A completed try_wait has reaped the child. Never signal its old
        // PID/process group, which the kernel may already have reused.
        if self.child.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if std::thread::panicking() {
            eprintln!(
                "noded personality evidence retained at {}",
                self.root.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

/// The broker itself answers, through the production native client — no
/// simulated broker, because the subject is the personality's real
/// `noded::main`: its runtime, config, logging identity and SIGTERM path
/// (`services/noded/tests/mesh_processes.rs` drives the standalone binary
/// the same way).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_noded_personality_brokers_and_stops_cleanly_on_sigterm() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("system-noded-{}-{nanos:x}", std::process::id()));
        fs::create_dir(&root).unwrap();
        // Native ingress serves separate UIDs and refuses non-traversable
        // existing ancestors; 0o711 keeps the socket path traversable.
        fs::set_permissions(&root, fs::Permissions::from_mode(0o711)).unwrap();
        for dir in ["etc", "var", "run", "home"] {
            fs::create_dir(root.join(dir)).unwrap();
        }

        // A reserved-then-released loopback port, so the broker never
        // touches a port another test or the host is using.
        let reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);

        let conf = json!({
            "node": "alpha",
            "wg_ip": "127.0.0.1",
            "noded": {"port": port, "unix_socket": root.join("run/bus.sock")}
        });
        fs::write(
            root.join("etc/node.conf.mix"),
            serde_json::to_vec_pretty(&conf).unwrap(),
        )
        .unwrap();

        symlink(BIN, root.join("noded")).unwrap();
        let log = fs::File::create(root.join("noded.log")).unwrap();
        let mut cmd = Command::new(root.join("noded"));
        cmd.args(["serve", "--no-monitor", "--no-log"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .env("MIXOS_ETC", root.join("etc"))
            .env("MIXOS_VAR", root.join("var"))
            .env("MIXOS_RUN", root.join("run"))
            .env("MIXOS_NODE_CONFIG", root.join("etc/node.conf.mix"))
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log));
        let child = cmd.spawn().unwrap();
        let mut proc = NodedPersonality {
            child,
            root: root.clone(),
        };

        // Readiness: keep trying the production Unix connect until the
        // broker answers, on a deadline, and fail fast if it died.
        let mut options = UnixConnectOptions::new(BrokerAccount {
            // SAFETY: process credentials, never an application-supplied
            // identity.
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
        });
        options.endpoint = Some(root.join("run/bus.sock"));
        options.require_native_session = true;
        let url = format!("ws://127.0.0.1:{port}/ws");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let connection = loop {
            assert!(
                proc.child.try_wait().unwrap().is_none(),
                "noded personality exited early; log: {}",
                root.display()
            );
            let error =
                match NodedClient::connect_unix("personality-probe", &url, &options, None).await {
                    Ok(UnixConnectOutcome::VerifiedUnix(connection)) => break connection,
                    Ok(_) => panic!("Unix verification cannot fall back to TCP"),
                    Err(error) => error,
                };
            assert!(
                tokio::time::Instant::now() < deadline,
                "broker readiness deadline: {error}; log: {}",
                root.display()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };

        // The real broker answers through the native production API.
        let pong = connection
            .client()
            .call("noded", "noded.ping", json!({}))
            .await
            .unwrap();
        assert_eq!(pong["pong"], true, "{pong}");
        connection.client().deregister().await.unwrap();

        // SIGTERM takes the daemon's own graceful path: stop, exit 0.
        unsafe {
            libc::kill(proc.child.id() as libc::pid_t, libc::SIGTERM);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = proc.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "noded personality did not stop on SIGTERM; log: {}",
                root.display()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(
            status.success(),
            "unclean shutdown {status}; log: {}",
            root.display()
        );
        // Daemon logs may go to journald instead of stderr. The captured
        // file is diagnostic evidence, not a required logging sink.
    })
    .await
    .expect("noded personality acceptance deadline");
}
