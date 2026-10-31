// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Contract 1.0.0 on real brokers (key space v2 §5.1, §5.4): change-only
//! per-prefix topics on one broker, then an authority and a mirror on a
//! private broker pair. Gated like the other broker tests: run with
//! `MIXOS_TEST_NODED` naming an exact-revision noded and `--ignored`.
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use bus::native_client::{IncomingCommand, NodedClient};
use ed25519_dalek::{Signer as _, SigningKey};
use mesh_trust::inventory::{
    ALG_ED25519, CANONICAL_ENCODING_V1, InvSignature, InventoryPayload, KeyStatus, SignedInventory,
    VerifyKey,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::sync::mpsc::UnboundedReceiver;

struct Process(Option<Child>);
impl Process {
    fn stop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    /// A clean stop: SIGTERM, as systemd stops the unit.
    fn terminate(&mut self) {
        if let Some(mut child) = self.0.take() {
            // SAFETY: signalling our own child by its pid.
            unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while child.try_wait().unwrap().is_none() {
                assert!(std::time::Instant::now() < deadline, "no clean stop");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Every 1.0.0 topic of the default profile.
fn topics() -> Vec<String> {
    let mut all: Vec<String> = settings::topics::PREFIXES
        .iter()
        .map(|p| settings::topics::keys_topic("default", p))
        .collect();
    all.push(settings::topics::look_topic("default"));
    all.push(settings::topics::snapshot_topic("default"));
    all
}

fn keys(prefix: &str) -> String {
    settings::topics::keys_topic("default", prefix)
}

fn snapshot() -> String {
    settings::topics::snapshot_topic("default")
}

/// A private broker: one node, its own roots, admission off.
struct Broker {
    root: PathBuf,
    ip: String,
    port: u16,
    process: Process,
}
impl Broker {
    fn url(&self) -> String {
        format!("ws://{}:{}/ws", self.ip, self.port)
    }
    fn start(&mut self) {
        assert!(self.process.0.is_none());
        let root = &self.root;
        let log = fs::File::options()
            .create(true)
            .append(true)
            .open(root.join("noded.log"))
            .unwrap();
        self.process = Process(Some(
            Command::new(std::env::var("MIXOS_TEST_NODED").expect("MIXOS_TEST_NODED"))
                .args(["serve", "--no-monitor", "--no-log"])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", root.join("home"))
                .env("MIXOS_ETC", root.join("etc"))
                .env("MIXOS_VAR", root.join("var"))
                .env("MIXOS_RUN", root.join("run"))
                .env("MIXOS_NODE_CONFIG", root.join("etc/node.conf.mix"))
                .env("RUST_LOG", "info")
                .stdout(Stdio::from(log.try_clone().unwrap()))
                .stderr(Stdio::from(log))
                .spawn()
                .unwrap(),
        ));
    }
    /// A settingsd on this broker.
    fn settingsd(&self, args: &[&str], log: &Path) -> Process {
        let log = fs::File::options()
            .create(true)
            .append(true)
            .open(log)
            .unwrap();
        Process(Some(
            Command::new(env!("CARGO_BIN_EXE_settingsd"))
                .args(args)
                .env("MIXOS_NODED_URL", self.url())
                .env("MIXOS_NODE_CONFIG", self.root.join("etc/node.conf.mix"))
                .env("RUST_LOG", "settingsd=info")
                .stdout(Stdio::from(log.try_clone().unwrap()))
                .stderr(Stdio::from(log))
                .spawn()
                .unwrap(),
        ))
    }
}

/// One broker, or a mesh of brokers that admit each other by a signed
/// inventory. Evidence is kept when a test fails.
struct Fixture {
    root: PathBuf,
    brokers: Vec<Broker>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for broker in &mut self.brokers {
            broker.process.stop();
        }
        if std::thread::panicking() {
            eprintln!("settings bus evidence retained at {}", self.root.display());
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
impl Fixture {
    fn new(names: &[&str]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "settingsd-bus-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7().simple()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o711)).unwrap();
        let ips: Vec<String> = (0..names.len())
            .map(|i| format!("127.0.0.{}", i + 2))
            .collect();
        let reservations: Vec<_> = ips
            .iter()
            .map(|ip| std::net::TcpListener::bind((ip.as_str(), 0)).unwrap())
            .collect();
        let ports: Vec<u16> = reservations
            .iter()
            .map(|l| l.local_addr().unwrap().port())
            .collect();
        let signing_key = SigningKey::from_bytes(&[42; 32]);
        let pubkey = B64.encode(signing_key.verifying_key().to_bytes());
        let members: Vec<Value> = names
            .iter()
            .zip(&ips)
            .zip(&ports)
            .map(|((name, ip), port)| {
                json!({"name":name, "mesh_ip":ip, "bus":true, "status":"active", "noded_port":port})
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
            hub: vec![names[0].into()],
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
        let mut brokers = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let broker_root = root.join(name);
            for dir in ["etc/noded", "var/noded", "run", "home"] {
                fs::create_dir_all(broker_root.join(dir)).unwrap();
            }
            fs::write(broker_root.join("etc/noded/genesis.pub"), &pubkey).unwrap();
            fs::write(
                broker_root.join("var/noded/inventory.signed"),
                serde_json::to_vec_pretty(&signed).unwrap(),
            )
            .unwrap();
            fs::write(
                broker_root.join("etc/node.conf.mix"),
                serde_json::to_vec_pretty(&json!({
                    "node": name, "wg_ip": ips[i],
                    "noded": {"port": ports[i], "admission": "off",
                        "unix_socket": broker_root.join("run/bus.sock")}
                }))
                .unwrap(),
            )
            .unwrap();
            fs::write(
                broker_root.join("etc/mesh.conf.mix"),
                serde_json::to_vec_pretty(&json!({"node_name": name, "peers": []})).unwrap(),
            )
            .unwrap();
            let mut broker = Broker {
                root: broker_root,
                ip: ips[i].clone(),
                port: ports[i],
                process: Process(None),
            };
            broker.start();
            brokers.push(broker);
        }
        Self { root, brokers }
    }

    /// A fresh profile for `instance`, made by the binary under test.
    fn profile(&self, instance: &str) -> PathBuf {
        let root = self.root.join(format!("profile-{instance}"));
        let status = Command::new(env!("CARGO_BIN_EXE_settingsd"))
            .args(["init", "--instance", instance, "--root"])
            .arg(&root)
            .status()
            .unwrap();
        assert!(status.success(), "settingsd init");
        root
    }
}

async fn connect(url: &str, name: &str) -> NodedClient {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match NodedClient::connect(name, url).await {
            Ok(client) => return client,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "broker at {url}: {error}"
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }
}

/// Call until it answers rc 0, then return the answer.
async fn ready(client: &NodedClient, to: &str, command: &str, body: Value) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match client.call(to, command, body.clone()).await {
            Ok(value) => return value,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "{to} {command}: {error}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// The raw answer: rc and the parsed body.
async fn raw(client: &NodedClient, to: &str, command: &str, body: &Value) -> (u8, Value) {
    let (rc, text, _) = client
        .call_with_headers_raw(to, command, &BTreeMap::new(), &body.to_string())
        .await
        .unwrap();
    (rc, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn binding(instance: &str) -> Value {
    json!({"instance": instance, "profile": "default"})
}

/// Wait until `settings.status` at `to` satisfies `done`.
async fn status_until(
    client: &NodedClient,
    to: &str,
    instance: &str,
    what: &str,
    done: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(status) = client
            .call(to, "settings.status", json!({"binding": binding(instance)}))
            .await
            && done(&status)
        {
            return status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "status never {what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A subscriber to every 1.0.0 topic: the latest body of each, and every
/// delivery in order.
struct Watch {
    client: NodedClient,
    events: UnboundedReceiver<IncomingCommand>,
    latest: BTreeMap<String, Value>,
    log: Vec<String>,
    /// Every delivery's topic and revision, retained replays included.
    history: Vec<(String, Value)>,
}
impl Watch {
    async fn new(url: &str, name: &str) -> Self {
        let client = connect(url, name).await;
        let events = client.incoming_async().await.unwrap();
        for topic in topics() {
            client
                .call_with_headers(
                    "noded",
                    "topic.subscribe",
                    &BTreeMap::from([("name".into(), topic)]),
                    "",
                )
                .await
                .unwrap();
        }
        Self {
            client,
            events,
            latest: BTreeMap::new(),
            log: Vec::new(),
            history: Vec::new(),
        }
    }
    fn take(&mut self, event: IncomingCommand) {
        let Some(topic) = event.topic().map(str::to_owned) else {
            return;
        };
        let body: Value = serde_json::from_str(&event.body).unwrap();
        assert_eq!(body["schema"], 3, "{topic}");
        self.log.push(topic.clone());
        self.history.push((topic.clone(), body["revision"].clone()));
        self.latest.insert(topic, body);
    }
    /// Take deliveries until `done` holds.
    async fn until(&mut self, what: &str, done: impl Fn(&BTreeMap<String, Value>) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !done(&self.latest) {
            let event = tokio::time::timeout_at(deadline, self.events.recv())
                .await
                .unwrap_or_else(|_| {
                    let seen: BTreeMap<&String, (&Value, &Value)> = self
                        .latest
                        .iter()
                        .map(|(t, b)| (t, (&b["revision"], &b["changed_at"])))
                        .collect();
                    panic!("never {what}: {seen:?}")
                })
                .unwrap();
            self.take(event);
        }
    }
    /// Take whatever else arrives within `quiet`.
    async fn settle(&mut self, quiet: Duration) {
        while let Ok(Some(event)) = tokio::time::timeout(quiet, self.events.recv()).await {
            self.take(event);
        }
    }
    fn since(&mut self) -> BTreeSet<String> {
        std::mem::take(&mut self.log).into_iter().collect()
    }
}

/// Every topic present, each at `revision`.
fn all_at(latest: &BTreeMap<String, Value>, revision: &Value) -> bool {
    topics()
        .iter()
        .all(|t| latest.get(t).is_some_and(|b| &b["revision"] == revision))
}

fn at(latest: &BTreeMap<String, Value>, topic: &str, revision: &Value) -> bool {
    latest
        .get(topic)
        .is_some_and(|b| &b["revision"] == revision)
}

fn apply(instance: &str, status: &Value, operation: &str, changes: Value) -> Value {
    json!({"binding": binding(instance), "expected_incarnation": status["incarnation"],
        "expected_revision": status["revision"], "operation_id": operation, "changes": changes})
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
#[ignore = "requires exact-revision noded (MIXOS_TEST_NODED)"]
fn a_batch_publishes_exactly_its_prefixes_and_a_reconnect_republishes_every_topic() {
    runtime().block_on(async {
        let mut fixture = Fixture::new(&["alpha"]);
        let profile = fixture.profile("alpha");
        let log = fixture.root.join("authority.log");
        let broker = &fixture.brokers[0];
        let _authority = broker.settingsd(
            &[
                "serve",
                "--instance",
                "alpha",
                "--root",
                profile.to_str().unwrap(),
            ],
            &log,
        );
        let mut watch = Watch::new(&broker.url(), "topic-observer").await;
        let status = ready(
            &watch.client,
            "settingsd",
            "settings.status",
            json!({"binding": binding("alpha")}),
        )
        .await;
        let first = status["revision"].clone();
        watch
            .until("every topic retained", |l| all_at(l, &first))
            .await;
        watch.settle(Duration::from_millis(300)).await;
        watch.since();

        // A batch of two prefixes publishes those two and the snapshot. The
        // look is untouched (neither key is in it), so it stays quiet.
        let request = apply(
            "alpha",
            &status,
            "batch-two-prefixes",
            json!({
            "time.applet.show_seconds": true, "accessibility.invert": true}),
        );
        let changed = watch
            .client
            .call("settingsd", "settings.apply", request)
            .await
            .unwrap();
        assert_eq!(changed["status"], "changed", "{changed}");
        let second = changed["receipt"]["revision"].clone();
        watch
            .until("the batch", |l| {
                at(l, &keys("time"), &second)
                    && at(l, &keys("accessibility"), &second)
                    && at(l, &snapshot(), &second)
            })
            .await;
        watch.settle(Duration::from_millis(1500)).await;
        assert_eq!(
            watch.since(),
            BTreeSet::from([keys("time"), keys("accessibility"), snapshot()]),
            "exactly the changed prefixes"
        );
        let time = &watch.latest[&keys("time")];
        assert_eq!(time["values"]["time.applet.show_seconds"], true);
        assert_eq!(time["changed_at"], second);
        assert_eq!(time["schema"], 3);
        let window = &watch.latest[&keys("window")];
        assert_eq!(
            window["changed_at"], first,
            "an untouched prefix keeps its stamp"
        );
        // Publication completes: the transitional topics mark the revision
        // published, so nothing stays pending and nothing republishes.
        let settled = status_until(&watch.client, "settingsd", "alpha", "published", |s| {
            s["publication_pending"] == false
        })
        .await;
        assert_eq!(settled["published_revision"], second);

        // A restarted broker has none of the retained state: a fresh
        // subscriber sees every topic only because settingsd republished.
        let before = watch.latest.clone();
        watch.client.close().await;
        fixture.brokers[0].process.stop();
        fixture.brokers[0].start();
        let broker = &fixture.brokers[0];
        let mut fresh = Watch::new(&broker.url(), "after-reconnect").await;
        fresh
            .until("every topic republished", |l| all_at(l, &second))
            .await;
        for topic in topics() {
            let (old, new) = (&before[&topic], &fresh.latest[&topic]);
            assert_eq!(old["changed_at"], new["changed_at"], "{topic}");
            assert_eq!(old["values"], new["values"], "{topic}");
        }

        // The validate preview moves by content: under `auto`, a `state`
        // change also moves `appearance` (its resolved mode) and the look.
        let status = ready(&fresh.client, "settingsd", "settings.status", json!({"binding": binding("alpha")})).await;
        let auto = apply("alpha", &status, "mode-auto", json!({"appearance.mode": "auto"}));
        let changed = fresh.client.call("settingsd", "settings.apply", auto).await.unwrap();
        assert_eq!(changed["status"], "changed", "{changed}");
        let status = ready(&fresh.client, "settingsd", "settings.status", json!({"binding": binding("alpha")})).await;
        let daylight = apply("alpha", &status, "preview", json!({"state.appearance.daylight": true}));
        let preview = fresh.client.call("settingsd", "settings.validate", daylight).await.unwrap();
        let moved: BTreeSet<&str> = preview["moved"].as_array().unwrap().iter().map(|m| m.as_str().unwrap()).collect();
        assert_eq!(moved, BTreeSet::from(["appearance", "look", "state"]), "{preview}");
        assert_eq!(preview["values"]["appearance"]["resolved"]["appearance.mode"], "light");
        fresh.client.close().await;
    });
}

#[test]
#[ignore = "requires exact-revision noded (MIXOS_TEST_NODED)"]
fn a_mirror_on_a_second_broker_republishes_forwards_serves_stale_and_catches_up() {
    runtime().block_on(async {
        let mut fixture = Fixture::new(&["alpha", "beta"]);
        let profile = fixture.profile("alpha");
        let mirror_root = fixture.root.join("mirror");
        let (authority_log, mirror_log) = (
            fixture.root.join("authority.log"),
            fixture.root.join("mirror.log"),
        );
        let authority_args = [
            "serve",
            "--instance",
            "alpha",
            "--root",
            profile.to_str().unwrap(),
        ];
        let mut authority = fixture.brokers[0].settingsd(&authority_args, &authority_log);
        let (alpha_url, beta_url) = (fixture.brokers[0].url(), fixture.brokers[1].url());
        let mut a = Watch::new(&alpha_url, "observer-alpha").await;
        let b_client = connect(&beta_url, "operator-beta").await;
        // The authority answers across the mesh before the mirror starts.
        ready(
            &b_client,
            "settingsd.alpha.bus",
            "settings.status",
            json!({"binding": binding("alpha")}),
        )
        .await;
        b_client.close().await;
        let _mirror = fixture.brokers[1].settingsd(
            &[
                "serve",
                "--instance",
                "alpha",
                "--mirror-of",
                "alpha",
                "--root",
                mirror_root.to_str().unwrap(),
            ],
            &mirror_log,
        );
        let mut b = Watch::new(&beta_url, "observer-beta").await;
        let status = status_until(&b.client, "settingsd", "alpha", "an attached mirror", |s| {
            s["role"] == "mirror" && s["stale"] == false
        })
        .await;
        assert_eq!(status["authority"], "alpha");
        let first = status["revision"].clone();
        let authority_status = ready(
            &a.client,
            "settingsd",
            "settings.status",
            json!({"binding": binding("alpha")}),
        )
        .await;
        assert_eq!(authority_status["role"], "authority");
        assert!(
            authority_status["mirrors"].get("beta").is_some(),
            "{authority_status}"
        );

        // The same topics at the same revision on both nodes.
        a.until("alpha's topics", |l| all_at(l, &first)).await;
        b.until("beta's topics", |l| all_at(l, &first)).await;
        assert_eq!(
            a.latest, b.latest,
            "the mirror's bodies are the authority's"
        );
        a.settle(Duration::from_millis(300)).await;
        b.settle(Duration::from_millis(300)).await;
        a.since();
        b.since();

        // Forged control traffic changes nothing. A validly sealed record at
        // a far higher revision, which would wedge the mirror if it were
        // taken, is refused from a local caller naming itself the authority
        // (the broker overwrites those headers) and from the authority's
        // node by a service that is not settingsd (or, where noded stamps no
        // service, because the authority does not hold it). An attach for the mirror's
        // node from elsewhere is refused too.
        let stored = fs::read_to_string(mirror_root.join("mirror.conf.mix")).unwrap();
        let mut forged: settingsd::mirror::Cache = strict::from_str(&stored).unwrap();
        forged.revision = settings::Revision(999);
        forged.design_revision = settings::Revision(999);
        forged.seal().unwrap();
        let push = json!({"binding": binding("alpha"), "authority": "alpha", "cache": forged,
            "changed": [], "changed_at": {}});
        let spoofed = BTreeMap::from([
            ("broker_peer".to_owned(), "alpha".to_owned()),
            ("broker_service".to_owned(), "settingsd".to_owned()),
            ("mesh_from".to_owned(), "settingsd".to_owned()),
        ]);
        let (rc, text, _) = b
            .client
            .call_with_headers_raw("settingsd", "settings.mirror.revision", &spoofed, &push.to_string())
            .await
            .unwrap();
        assert!(rc >= 10 && text.contains("wrong_sender"), "{rc} {text}");
        // From the authority's node: refused by the stamped service where the
        // broker stamps one (wrong_sender), else by the authority itself,
        // which does not hold that record (not_confirmed).
        let (rc, body) = raw(&a.client, "settingsd.beta.bus", "settings.mirror.revision", &push).await;
        assert!(rc >= 10, "{rc} {body}");
        assert!(
            body["status"] == "wrong_sender" || body["status"] == "not_confirmed",
            "{body}"
        );
        let mut stop = push.clone();
        stop["stopping"] = json!(true);
        let (rc, body) = raw(&b.client, "settingsd", "settings.mirror.revision", &stop).await;
        assert_eq!((rc >= 10, &body["status"]), (true, &json!("wrong_sender")), "{body}");
        let attach = json!({"binding": binding("alpha"), "node": "beta", "revision": "1"});
        let (rc, body) = raw(&a.client, "settingsd", "settings.mirror.attach", &attach).await;
        assert_eq!((rc >= 10, &body["status"]), (true, &json!("wrong_sender")), "{body}");
        let unmoved = status_until(&b.client, "settingsd", "alpha", "unmoved", |_| true).await;
        assert_eq!(unmoved["revision"], first, "the forged record was not taken");
        assert_eq!(unmoved["stale"], false, "nor did it touch stale");

        // An authority commit: the mirror republishes the same topics (and so
        // was not wedged by the forged revision).
        let request = apply(
            "alpha",
            &authority_status,
            "on-the-authority",
            json!({"time.applet.show_seconds": true}),
        );
        let changed = a
            .client
            .call("settingsd", "settings.apply", request)
            .await
            .unwrap();
        let second = changed["receipt"]["revision"].clone();
        let moved = |l: &BTreeMap<String, Value>| {
            at(l, &keys("time"), &second) && at(l, &snapshot(), &second)
        };
        a.until("the commit on alpha", moved).await;
        b.until("the commit on beta", moved).await;
        a.settle(Duration::from_millis(1000)).await;
        b.settle(Duration::from_millis(1000)).await;
        let expected = BTreeSet::from([keys("time"), snapshot()]);
        assert_eq!(a.since(), expected);
        assert_eq!(
            b.since(),
            expected,
            "the mirror republishes only what moved"
        );
        assert_eq!(a.latest, b.latest);

        // A write through the mirror gets the authority's receipt, and its
        // replay by operation_id returns the same one.
        let mirror_status = status_until(&b.client, "settingsd", "alpha", "current", |s| {
            s["revision"] == second
        })
        .await;
        let request = apply(
            "alpha",
            &mirror_status,
            "through-the-mirror",
            json!({"accessibility.invert": true}),
        );
        let through = b
            .client
            .call("settingsd", "settings.apply", request.clone())
            .await
            .unwrap();
        assert_eq!(through["status"], "changed", "{through}");
        assert_eq!(through["replayed"], false);
        let third = through["receipt"]["revision"].clone();
        let replay = b
            .client
            .call("settingsd", "settings.apply", request.clone())
            .await
            .unwrap();
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["receipt"], through["receipt"]);
        let receipt = a
            .client
            .call(
                "settingsd",
                "settings.status",
                json!({"binding": binding("alpha"), "operation_id": "through-the-mirror"}),
            )
            .await
            .unwrap();
        assert_eq!(
            receipt["receipt"], through["receipt"],
            "the authority holds the receipt"
        );
        let moved = |l: &BTreeMap<String, Value>| at(l, &keys("accessibility"), &third);
        a.until("the mirrored write on alpha", moved).await;
        b.until("the mirrored write on beta", moved).await;
        assert_eq!(
            a.latest[&keys("accessibility")],
            b.latest[&keys("accessibility")]
        );

        // The authority stops: the mirror serves its cache stale and refuses
        // writes with authority_unreachable.
        authority.terminate();
        status_until(&b.client, "settingsd", "alpha", "stale", |s| {
            s["stale"] == true
        })
        .await;
        let cached = b
            .client
            .call(
                "settingsd",
                "settings.get",
                json!({"binding": binding("alpha"), "view": "values", "prefix": "accessibility"}),
            )
            .await
            .unwrap();
        assert_eq!(cached["revision"], third);
        assert_eq!(cached["values"]["accessibility.invert"], true);
        let mut refused = request.clone();
        refused["operation_id"] = json!("while-down");
        refused["expected_revision"] = third.clone();
        let (rc, body) = raw(&b.client, "settingsd", "settings.apply", &refused).await;
        assert!(rc >= 10, "{rc} {body}");
        assert_eq!(body["status"], "authority_unreachable", "{body}");
        assert_eq!(body["node"], "alpha");

        // The authority comes back: it pushes to the mirror it had attached.
        let mut authority = fixture.brokers[0].settingsd(&authority_args, &authority_log);
        status_until(&b.client, "settingsd", "alpha", "attached again", |s| {
            s["stale"] == false
        })
        .await;

        // A crash (kill -9) sends no notice and the mesh has no peer-down
        // event to tell the mirror: it still reports current, until its next
        // forwarded write finds the authority gone, refuses it, and goes
        // stale.
        authority.stop();
        let before = status_until(&b.client, "settingsd", "alpha", "read", |_| true).await;
        assert_eq!(before["stale"], false, "no event reaches the mirror");
        refused["operation_id"] = json!("after-the-crash");
        let (rc, body) = raw(&b.client, "settingsd", "settings.apply", &refused).await;
        assert!(rc >= 10, "{rc} {body}");
        assert_eq!(body["status"], "authority_unreachable", "{body}");
        status_until(&b.client, "settingsd", "alpha", "stale after the refusal", |s| {
            s["stale"] == true
        })
        .await;
        let _authority = fixture.brokers[0].settingsd(&authority_args, &authority_log);
        status_until(&b.client, "settingsd", "alpha", "attached after the crash", |s| {
            s["stale"] == false
        })
        .await;

        // The mirror's own broker restarts while the authority commits: on
        // its reconnect the mirror re-attaches and catches up.
        b.client.close().await;
        fixture.brokers[1].process.stop();
        let current = ready(
            &a.client,
            "settingsd",
            "settings.status",
            json!({"binding": binding("alpha")}),
        )
        .await;
        let request = apply(
            "alpha",
            &current,
            "while-the-mirror-is-away",
            json!({"time.applet.show_date": false}),
        );
        let changed = a
            .client
            .call("settingsd", "settings.apply", request)
            .await
            .unwrap();
        let fourth = changed["receipt"]["revision"].clone();
        a.until("the commit while away", |l| at(l, &keys("time"), &fourth))
            .await;
        fixture.brokers[1].start();
        let mut b = Watch::new(&beta_url, "observer-beta-again").await;
        status_until(&b.client, "settingsd", "alpha", "caught up", |s| {
            s["revision"] == fourth && s["stale"] == false
        })
        .await;
        // A reconnect republishes every topic, at the revision the attach
        // brought, not the cache's.
        b.until("beta's topics after its reconnect", |l| all_at(l, &fourth))
            .await;
        // And the cache's revision never went out first: publication is held
        // while the attach is in flight.
        let early: Vec<_> = b.history.iter().filter(|(_, r)| *r == third).collect();
        assert!(early.is_empty(), "the stale cache was published: {early:?}");
        assert_eq!(a.latest[&keys("time")], b.latest[&keys("time")]);
        let mirror_log = fs::read_to_string(&mirror_log).unwrap();
        assert!(
            mirror_log.contains("mirror took the authority's revision"),
            "{mirror_log}"
        );
        a.client.close().await;
        b.client.close().await;
    });
}
