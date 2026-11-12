// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! What a broker costs. Real `noded` processes in a signed fixture mesh, with
//! live links in both directions and a service on every node; the first
//! broker is measured from `/proc`.
//!
//! `idle_broker_stays_within_its_wakeup_budget` is the gate: an idle broker
//! wakes for real timers and real I/O only.
//!
//! `profile` is ignored and prints one `PROFILE {json}` line per phase (idle,
//! registration churn, two local bursts, a mesh burst, a flapping peer, three
//! rounds of requests nobody answers, idle again):
//!
//! ```text
//! cargo test --release -p noded --test efficiency -- --ignored --nocapture profile
//! ```
//!
//! It reads `NODED_PROFILE_PEERS` (default 4, at most 4),
//! `NODED_PROFILE_IDLE_SECS` (20), `NODED_PROFILE_CALLS` (200000),
//! `NODED_PROFILE_REGISTRATIONS` (2000), `NODED_PROFILE_FLAPS` (5) and
//! `NODED_PROFILE_UNANSWERED` (50000 a round).
//! `NODED_PROFILE_STRACE=1` runs the measured broker under `strace` and adds
//! its system calls per phase; the CPU figures of that run mean nothing.
//! `NODED_PROFILE_BIN` names the broker to run instead of the one cargo
//! built, so a prebuilt test binary can be run on a host that has strace.
//! `scripts/idle_profile.mix` drives both runs and takes the same idle
//! numbers from a live process.
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use bus::native_client::{
    BrokerAccount, NodedClient, UnixConnectOptions, UnixConnectOutcome, VerifiedConnection,
};
use ed25519_dalek::{Signer as _, SigningKey};
use mesh_trust::inventory::{
    ALG_ED25519, CANONICAL_ENCODING_V1, InvSignature, InventoryPayload, KeyStatus, SignedInventory,
    VerifyKey,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Wake-ups a second allowed to an idle broker with live mesh links. The
/// broker's own timers are a 1 s publish flush and a 10 s janitor, and it
/// measures 2.3 a second; the regression this guards (a 2 ms drain tick)
/// made 500. Anything polling faster than every 50 ms fails.
const IDLE_WAKEUP_BUDGET: f64 = 20.0;

const NAMES: [&str; 5] = ["alpha", "beta", "gamma", "delta", "epsilon"];

struct Node {
    name: &'static str,
    root: PathBuf,
    url: String,
    child: Option<Child>,
    /// The broker itself; differs from `child` when strace is the parent.
    pid: u32,
}

impl Node {
    fn start(&mut self, strace: bool) {
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("noded.log"))
            .unwrap();
        let mut command = if strace {
            let mut command = Command::new("strace");
            command
                .args(["-f", "-ttt", "-o"])
                .arg(self.root.join("strace.log"))
                .arg(noded_bin());
            command
        } else {
            Command::new(noded_bin())
        };
        let child = command
            .args(["serve", "--no-monitor", "--no-log"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.join("home"))
            .env("MIXOS_ETC", self.root.join("etc"))
            .env("MIXOS_VAR", self.root.join("var"))
            .env("MIXOS_RUN", self.root.join("run"))
            .env("MIXOS_NODE_CONFIG", self.root.join("etc/node.conf.mix"))
            .env("RUST_LOG", "info")
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap();
        self.pid = if strace {
            child_of(child.id())
        } else {
            child.id()
        };
        self.child = Some(child);
    }

    /// SIGTERM the broker and wait, so a tracing parent flushes and exits.
    fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // SAFETY: signals a process this fixture started and has not reaped.
        unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        // SAFETY: as above; a no-op once the broker has gone.
        unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGKILL) };
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The broker under test: the one cargo built beside this test, unless
/// `NODED_PROFILE_BIN` names another (a prebuilt pair run on another host).
fn noded_bin() -> PathBuf {
    std::env::var_os("NODED_PROFILE_BIN")
        .map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_noded")), PathBuf::from)
}

struct Mesh {
    root: PathBuf,
    nodes: Vec<Node>,
}

impl Drop for Mesh {
    fn drop(&mut self) {
        for node in &mut self.nodes {
            node.stop();
        }
        if std::thread::panicking() {
            eprintln!("efficiency evidence retained at {}", self.root.display());
        } else {
            fs::remove_dir_all(&self.root).expect("remove owned fixture root");
        }
    }
}

fn write(path: &Path, value: &impl serde::Serialize) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

impl Mesh {
    /// `count` brokers on loopback addresses under one signed inventory.
    fn start(count: usize, strace_first: bool) -> Self {
        assert!((2..=NAMES.len()).contains(&count));
        let root = std::env::temp_dir().join(format!(
            "noded-eff-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir(&root).unwrap();
        // Native ingress refuses non-traversable ancestors.
        fs::set_permissions(&root, fs::Permissions::from_mode(0o711)).unwrap();
        let ips: Vec<String> = (0..count).map(|i| format!("127.0.0.{}", i + 2)).collect();
        let reservations: Vec<_> = ips
            .iter()
            .map(|ip| TcpListener::bind((ip.as_str(), 0)).unwrap())
            .collect();
        let ports: Vec<u16> = reservations
            .iter()
            .map(|listener| listener.local_addr().unwrap().port())
            .collect();
        let signing_key = SigningKey::from_bytes(&rand::random::<[u8; 32]>());
        let pubkey = B64.encode(signing_key.verifying_key().to_bytes());
        let members: Vec<Value> = (0..count)
            .map(|i| {
                json!({"name": NAMES[i], "mesh_ip": ips[i], "bus": true,
                       "status": "active", "noded_port": ports[i]})
            })
            .collect();
        let payload = InventoryPayload {
            schema_version: 1,
            canonical_encoding: CANONICAL_ENCODING_V1.into(),
            mesh: "example.test".into(),
            subnet: "127.0.0.0/24".into(),
            epoch: 1,
            signed_at: chrono::Utc::now().to_rfc3339(),
            valid_until: (chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339(),
            hub: vec![NAMES[0].into()],
            verify_keys: vec![VerifyKey {
                key_id: "genesis".into(),
                pubkey: pubkey.clone(),
                key_type: ALG_ED25519.into(),
                status: KeyStatus::Active,
            }],
            members: Value::Array(members),
            recovery: None,
            recovery_generation: None,
        };
        let signed = SignedInventory {
            signatures: vec![InvSignature {
                key_id: "genesis".into(),
                alg: ALG_ED25519.into(),
                sig: B64.encode(signing_key.sign(&payload.canonical_bytes()).to_bytes()),
            }],
            payload,
        };
        drop(reservations);
        let mut mesh = Self {
            root,
            nodes: Vec::new(),
        };
        for i in 0..count {
            let root = mesh.root.join(NAMES[i]);
            for dir in ["etc/noded", "var/noded", "run", "home"] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            fs::write(root.join("etc/noded/genesis.pub"), &pubkey).unwrap();
            write(&root.join("var/noded/inventory.signed"), &signed);
            write(
                &root.join("etc/node.conf.mix"),
                &json!({
                    "node": NAMES[i], "wg_ip": ips[i],
                    "noded": {"port": ports[i], "unix_socket": root.join("run/bus.sock")}
                }),
            );
            write(
                &root.join("etc/mesh.conf.mix"),
                &json!({"node_name": NAMES[i], "peers": []}),
            );
            let mut node = Node {
                name: NAMES[i],
                url: format!("ws://{}:{}/ws", ips[i], ports[i]),
                root,
                child: None,
                pid: 0,
            };
            node.start(strace_first && i == 0);
            mesh.nodes.push(node);
        }
        mesh
    }
}

/// The traced broker: the first child of the strace process.
fn child_of(parent: u32) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        for entry in fs::read_dir("/proc").unwrap().flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            let ppid = stat
                .rsplit_once(") ")
                .and_then(|(_, tail)| tail.split(' ').nth(1))
                .and_then(|field| field.parse::<u32>().ok());
            if ppid == Some(parent) {
                return pid;
            }
        }
        assert!(Instant::now() < deadline, "strace started no broker");
        std::thread::sleep(Duration::from_millis(10));
    }
}

async fn connect(node: &Node, name: &str) -> Arc<VerifiedConnection> {
    let mut options = UnixConnectOptions::new(BrokerAccount {
        // SAFETY: process credentials, never an application-supplied identity.
        uid: unsafe { libc::geteuid() },
        gid: unsafe { libc::getegid() },
    });
    options.endpoint = Some(node.root.join("run/bus.sock"));
    options.default_endpoints = Vec::new();
    options.require_native_session = true;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let error = match NodedClient::connect_unix(name, &node.url, &options, None).await {
            Ok(UnixConnectOutcome::VerifiedUnix(connection)) => return Arc::new(connection),
            Ok(_) => panic!("Unix verification cannot fall back to TCP"),
            Err(error) => error,
        };
        assert!(
            tokio::time::Instant::now() < deadline,
            "broker {} readiness deadline: {error}",
            node.name
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// An `echo` service: answers every request with its own body.
async fn serve_echo(node: &Node) -> Arc<VerifiedConnection> {
    let connection = connect(node, "echo").await;
    let service = connection.clone();
    tokio::spawn(async move {
        while let Some(delivery) = service.recv_shared().await {
            let command = delivery.command();
            let _ = service.client().respond(command, 0, &command.body).await;
        }
    });
    connection
}

/// Call until the route answers; returns the attempts made.
async fn call_until_ok(caller: &VerifiedConnection, target: &str) -> u64 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut attempts = 0;
    loop {
        attempts += 1;
        let reply = tokio::time::timeout(
            Duration::from_secs(2),
            caller
                .client()
                .call(target, "echo.tag", json!({"tag": "up"})),
        )
        .await;
        match reply {
            Ok(Ok(_)) => return attempts,
            other => assert!(
                tokio::time::Instant::now() < deadline,
                "{target} never answered: {other:?}"
            ),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Every connection the idle state consists of; dropping one closes it.
struct Links {
    echoes: Vec<Arc<VerifiedConnection>>,
    callers: Vec<Arc<VerifiedConnection>>,
}

/// An echo service and a caller on every node, and a link in each direction
/// between the first broker and each peer.
async fn link_up(mesh: &Mesh) -> Links {
    let mut links = Links {
        echoes: Vec::new(),
        callers: Vec::new(),
    };
    for node in &mesh.nodes {
        links.echoes.push(serve_echo(node).await);
        links.callers.push(connect(node, "caller").await);
    }
    for index in 1..mesh.nodes.len() {
        let outbound = format!("echo.{}.bus", mesh.nodes[index].name);
        call_until_ok(&links.callers[0], &outbound).await;
        let inbound = format!("echo.{}.bus", mesh.nodes[0].name);
        call_until_ok(&links.callers[index], &inbound).await;
    }
    links
}

#[derive(Clone, Copy)]
struct Sample {
    at: Instant,
    ticks: u64,
    voluntary: u64,
    involuntary: u64,
    rss_kb: u64,
    threads: u64,
}

fn status_field(status: &str, name: &str) -> u64 {
    status
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn sample(pid: u32) -> Sample {
    let root = PathBuf::from(format!("/proc/{pid}"));
    let stat = fs::read_to_string(root.join("stat")).expect("measured broker is running");
    // utime and stime, counted from the last ")" of the command name.
    let fields: Vec<&str> = stat
        .rsplit_once(") ")
        .map(|(_, tail)| tail.split(' ').collect())
        .unwrap_or_default();
    let ticks = fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap();
    let (mut voluntary, mut involuntary, mut threads) = (0, 0, 0);
    for task in fs::read_dir(root.join("task")).unwrap().flatten() {
        // A thread may exit between the listing and the read.
        let Ok(status) = fs::read_to_string(task.path().join("status")) else {
            continue;
        };
        threads += 1;
        voluntary += status_field(&status, "voluntary_ctxt_switches");
        involuntary += status_field(&status, "nonvoluntary_ctxt_switches");
    }
    let status = fs::read_to_string(root.join("status")).unwrap();
    Sample {
        at: Instant::now(),
        ticks,
        voluntary,
        involuntary,
        rss_kb: status_field(&status, "VmRSS"),
        threads,
    }
}

fn epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

struct Phase {
    name: &'static str,
    messages: u64,
    before: Sample,
    after: Sample,
    wall: (f64, f64),
}

impl Phase {
    async fn run(name: &'static str, pid: u32, work: impl Future<Output = u64>) -> Self {
        let wall_start = epoch();
        let before = sample(pid);
        let messages = work.await;
        Self {
            name,
            messages,
            before,
            after: sample(pid),
            wall: (wall_start, epoch()),
        }
    }

    fn seconds(&self) -> f64 {
        (self.after.at - self.before.at).as_secs_f64()
    }

    fn wakeups_per_s(&self) -> f64 {
        (self.after.voluntary - self.before.voluntary) as f64 / self.seconds()
    }

    fn report(&self, syscalls: Option<&BTreeMap<String, u64>>) -> Value {
        // SAFETY: sysconf reads a constant.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        let cpu_ms = (self.after.ticks - self.before.ticks) as f64 * 1000.0 / hz;
        let seconds = self.seconds();
        let mut top: Vec<(&String, &u64)> = syscalls.into_iter().flatten().collect();
        top.sort_by(|a, b| b.1.cmp(a.1));
        json!({
            "phase": self.name,
            "seconds": (seconds * 100.0).round() / 100.0,
            "cpu_ms": cpu_ms,
            "cpu_percent": (cpu_ms / seconds / 10.0 * 100.0).round() / 100.0,
            "wakeups_per_s": (self.wakeups_per_s() * 10.0).round() / 10.0,
            "involuntary_per_s":
                ((self.after.involuntary - self.before.involuntary) as f64 / seconds * 10.0).round() / 10.0,
            "rss_kb": self.after.rss_kb,
            "threads": self.after.threads,
            "messages": self.messages,
            "cpu_us_per_message":
                (self.messages > 0).then(|| (cpu_ms * 1000.0 / self.messages as f64 * 10.0).round() / 10.0),
            "syscalls": syscalls.map(|calls| calls.values().sum::<u64>()),
            "syscalls_per_s": syscalls
                .map(|calls| (calls.values().sum::<u64>() as f64 / seconds * 10.0).round() / 10.0),
            "top_syscalls": syscalls.map(|_| top.iter().take(6)
                .map(|(name, count)| format!("{name} {count}")).collect::<Vec<_>>()),
        })
    }
}

/// System calls made inside `[start, end]`, from `strace -f -ttt -o` lines:
/// `PID EPOCH.MICROS name(args) = result`. A call is counted where it starts.
fn syscalls_between(log: &str, start: f64, end: f64) -> BTreeMap<String, u64> {
    let mut calls = BTreeMap::new();
    for line in log.lines() {
        let mut fields = line.splitn(3, ' ');
        let (Some(_pid), Some(stamp), Some(rest)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Ok(stamp) = stamp.parse::<f64>() else {
            continue;
        };
        if stamp < start || stamp > end || rest.starts_with(['<', '+', '-']) {
            continue;
        }
        if let Some((name, _)) = rest.split_once('(') {
            *calls.entry(name.to_owned()).or_insert(0) += 1;
        }
    }
    calls
}

fn setting(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .map(|value| value.parse().expect("numeric profile setting"))
        .unwrap_or(default)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_broker_stays_within_its_wakeup_budget() {
    tokio::time::timeout(Duration::from_secs(90), async {
        let mesh = Mesh::start(3, false);
        let _links = link_up(&mesh).await;
        // Start-up work (route install, world publish, link admission) settles.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let idle = Phase::run("idle", mesh.nodes[0].pid, async {
            tokio::time::sleep(Duration::from_secs(4)).await;
            0
        })
        .await;
        println!("PROFILE {}", idle.report(None));
        assert!(
            idle.wakeups_per_s() <= IDLE_WAKEUP_BUDGET,
            "an idle broker made {:.1} wake-ups a second (budget {IDLE_WAKEUP_BUDGET}): \
             something polls where it should wait",
            idle.wakeups_per_s()
        );
    })
    .await
    .expect("idle budget deadline");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement, not a gate: run it by name with --nocapture"]
async fn profile() {
    let peers = setting("NODED_PROFILE_PEERS", 4) as usize;
    let idle_secs = setting("NODED_PROFILE_IDLE_SECS", 20);
    let calls = setting("NODED_PROFILE_CALLS", 200_000);
    let registrations = setting("NODED_PROFILE_REGISTRATIONS", 2_000);
    let flaps = setting("NODED_PROFILE_FLAPS", 5);
    let unanswered = setting("NODED_PROFILE_UNANSWERED", 50_000);
    let strace = std::env::var_os("NODED_PROFILE_STRACE").is_some();
    const CALLERS: u64 = 8;
    // Under the broker's 256-message connection queue, so every request is
    // delivered and waits for an answer instead of being refused.
    const UNANSWERED_BATCH: u64 = 100;

    tokio::time::timeout(Duration::from_secs(900), async {
        let mut mesh = Mesh::start(peers + 1, strace);
        let mut links = link_up(&mesh).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let pid = mesh.nodes[0].pid;
        let mut phases = Vec::new();
        let idle = |name| {
            Phase::run(name, pid, async move {
                tokio::time::sleep(Duration::from_secs(idle_secs)).await;
                0
            })
        };

        phases.push(idle("idle").await);

        phases.push(
            Phase::run("register", pid, async {
                for index in 0..registrations {
                    let service = connect(&mesh.nodes[0], &format!("churn-{index}")).await;
                    service.client().deregister().await.unwrap();
                    service.client().close().await;
                }
                registrations
            })
            .await,
        );

        let mut burst_callers = Vec::new();
        for index in 0..CALLERS {
            burst_callers.push(connect(&mesh.nodes[0], &format!("burst-{index}")).await);
        }
        // The repeated local burst separates memory an allocator keeps after
        // a burst (level after the second) from memory that grows per burst.
        for (name, target) in [
            ("local-burst", "echo".to_owned()),
            ("local-again", "echo".to_owned()),
            ("mesh-burst", format!("echo.{}.bus", mesh.nodes[1].name)),
        ] {
            let burst = Phase::run(name, pid, async {
                let tasks: Vec<_> = burst_callers
                    .iter()
                    .cloned()
                    .map(|caller| {
                        let target = target.clone();
                        tokio::spawn(async move {
                            for index in 0..calls / CALLERS {
                                caller
                                    .client()
                                    .call(&target, "echo.tag", json!({"n": index}))
                                    .await
                                    .unwrap();
                            }
                        })
                    })
                    .collect();
                for task in tasks {
                    task.await.unwrap();
                }
                calls / CALLERS * CALLERS
            })
            .await;
            phases.push(burst);
        }
        for caller in burst_callers {
            caller.client().close().await;
        }

        let flap_target = format!("echo.{}.bus", mesh.nodes[1].name);
        let mut attempts = 0;
        let before = sample(pid);
        let wall_start = epoch();
        for _ in 0..flaps {
            mesh.nodes[1].stop();
            let down = tokio::time::timeout(
                Duration::from_secs(2),
                links.callers[0]
                    .client()
                    .call(&flap_target, "echo.tag", json!({"tag": "down"})),
            )
            .await;
            assert!(!matches!(down, Ok(Ok(_))), "a stopped peer answered");
            attempts += 1;
            mesh.nodes[1].start(false);
            links.echoes[1] = serve_echo(&mesh.nodes[1]).await;
            attempts += call_until_ok(&links.callers[0], &flap_target).await;
        }
        phases.push(Phase {
            name: "peer-flap",
            messages: attempts,
            before,
            after: sample(pid),
            wall: (wall_start, epoch()),
        });

        // Requests nobody answers, from a caller that stays connected, to a
        // service that then goes away. Three equal rounds: memory that the
        // broker cannot reclaim climbs by the same step each time.
        for name in ["unanswered-1", "unanswered-2", "unanswered-3"] {
            let round = Phase::run(name, pid, async {
                let void = connect(&mesh.nodes[0], "void").await;
                let mut sent = 0;
                while sent < unanswered {
                    let batch: Vec<_> = (0..UNANSWERED_BATCH)
                        .map(|index| {
                            let caller = links.callers[0].clone();
                            tokio::spawn(async move {
                                let _ = tokio::time::timeout(
                                    Duration::from_millis(5),
                                    caller
                                        .client()
                                        .call("void", "void.wait", json!({"n": index})),
                                )
                                .await;
                            })
                        })
                        .collect();
                    for task in batch {
                        task.await.unwrap();
                    }
                    sent += UNANSWERED_BATCH;
                }
                void.client().close().await;
                tokio::time::sleep(Duration::from_secs(1)).await;
                sent
            })
            .await;
            phases.push(round);
        }

        phases.push(idle("idle-after").await);

        let trace = strace.then(|| {
            mesh.nodes[0].stop();
            fs::read_to_string(mesh.nodes[0].root.join("strace.log")).unwrap()
        });
        for phase in &phases {
            let syscalls = trace
                .as_deref()
                .map(|log| syscalls_between(log, phase.wall.0, phase.wall.1));
            println!("PROFILE {}", phase.report(syscalls.as_ref()));
        }
    })
    .await
    .expect("profile deadline");
}
