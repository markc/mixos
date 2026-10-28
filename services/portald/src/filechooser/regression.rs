// SPDX-License-Identifier: MIT OR Apache-2.0
//! Deterministic bridge cut points, with real D-Bus objects and native RPCs.
use super::*;
use futures_util::StreamExt;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex as SyncMutex;
use zbus::{MatchRule, MessageStream};

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Clone, Copy)]
enum Answer {
    Normal,
    Slow,
    Refuse,
    Silent,
}

struct Fixture {
    _daemon: Daemon,
    _dir: tempfile::TempDir,
    _broker: test_broker::Broker,
    server: Connection,
    owner: Connection,
    bridge: Bridge,
    provider: Arc<SupervisedClient>,
    worker: tokio::task::JoinHandle<()>,
    lists: Arc<SyncMutex<VecDeque<Answer>>>,
    cancels: Arc<SyncMutex<VecDeque<Answer>>>,
    cancelled: mpsc::UnboundedReceiver<String>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut daemon = Daemon(
            Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
                .arg(format!("--address=unix:path={}/bus", dir.path().display()))
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut address = String::new();
        BufReader::new(daemon.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let server = zbus::connection::Builder::address(address.trim())
            .unwrap()
            .build()
            .await
            .unwrap();
        let owner = zbus::connection::Builder::address(address.trim())
            .unwrap()
            .build()
            .await
            .unwrap();
        let broker = test_broker::Broker::start();
        let provider = Arc::new(
            SupervisedClient::connect_options("files", &broker.url)
                .with_commands(
                    ["files.choose.list", "files.choose.cancel"]
                        .into_iter()
                        .map(|name| bus::CommandDescriptor::new(name, &[], "test", false))
                        .collect(),
                )
                .connect()
                .await
                .unwrap(),
        );
        let native = Arc::new(
            SupervisedClient::connect_options("portal-regression", &broker.url)
                .connect()
                .await
                .unwrap(),
        );
        let bridge = Bridge::new(native);
        bridge.registry.lock().await.generation = settings::native::live_generation(&bridge.client);
        let lists = Arc::new(SyncMutex::new(VecDeque::new()));
        let cancels = Arc::new(SyncMutex::new(VecDeque::new()));
        let mut incoming = provider.incoming().unwrap();
        let (cancelled_tx, cancelled) = mpsc::unbounded_channel();
        let (p, l, c) = (provider.clone(), lists.clone(), cancels.clone());
        let worker = tokio::spawn(async move {
            while let Some(command) = incoming.recv().await {
                let answer = if command.command == "files.choose.cancel" {
                    let body: Json = serde_json::from_str(&command.body).unwrap();
                    cancelled_tx
                        .send(body["request"].as_str().unwrap().to_owned())
                        .unwrap();
                    c.lock().unwrap().pop_front().unwrap_or(Answer::Normal)
                } else {
                    l.lock().unwrap().pop_front().unwrap_or(Answer::Normal)
                };
                let p = p.clone();
                tokio::spawn(async move {
                    if matches!(answer, Answer::Silent) {
                        return;
                    }
                    if matches!(answer, Answer::Slow) {
                        tokio::time::sleep(IO_TIMEOUT + Duration::from_millis(200)).await;
                    }
                    let rc = if matches!(answer, Answer::Refuse) {
                        10
                    } else {
                        0
                    };
                    let value = if command.command == "files.choose.list" {
                        json!({"incarnation":"inc", "requests":[{"request":"inc-1"}]})
                    } else {
                        json!({})
                    };
                    let _ = p.respond(&command, rc, &value.to_string()).await;
                });
            }
        });
        Self {
            _daemon: daemon,
            _dir: dir,
            _broker: broker,
            server,
            owner,
            bridge,
            provider,
            worker,
            lists,
            cancels,
            cancelled,
        }
    }

    async fn entry(&self, phase: Phase) -> (OwnedObjectPath, MessageStream) {
        let owner = self.owner.unique_name().unwrap().as_str();
        let path = handle::request_path(owner, "regression").unwrap();
        let mut state = State::default();
        state.phase = phase;
        self.bridge.registry.lock().await.entries.insert(
            1,
            Entry {
                path: path.clone(),
                owner: owner.into(),
                choices: false,
                state,
                reply_sent: None,
            },
        );
        self.server
            .object_server()
            .at(
                path.clone(),
                request::Request::filechooser(owner, self.bridge.clone(), 1),
            )
            .await
            .unwrap();
        let stream = MessageStream::for_match_rule(
            MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("org.freedesktop.portal.Request")
                .unwrap()
                .member("Response")
                .unwrap()
                .path(path.clone())
                .unwrap()
                .build(),
            &self.owner,
            Some(4),
        )
        .await
        .unwrap();
        (path, stream)
    }

    async fn removed(&self, path: &OwnedObjectPath) {
        assert!(
            self.server
                .object_server()
                .interface::<_, request::Request>(path.clone())
                .await
                .is_err()
        );
        assert!(self.bridge.registry.lock().await.entries.is_empty());
    }

    async fn cancelled(&mut self) {
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), self.cancelled.recv())
                .await
                .unwrap()
                .unwrap(),
            "inc-1"
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if self
                    .bridge
                    .cancels
                    .lock()
                    .unwrap()
                    .values()
                    .all(|c| !c.in_flight)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}

async fn response(stream: &mut MessageStream) {
    let message = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (code, results): (u32, HashMap<String, zbus::zvariant::OwnedValue>) =
        message.body().deserialize().unwrap();
    assert_eq!(code, 2);
    assert!(results.is_empty());
    quiet(stream).await;
}

async fn quiet(stream: &mut MessageStream) {
    assert!(
        tokio::time::timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn open_file_during_reconnect_answers_once_and_unexports() {
    let f = Fixture::new().await;
    f.bridge.registry.lock().await.generation = None;
    f.server
        .object_server()
        .at(
            OBJECT_PATH,
            FileChooser {
                bridge: f.bridge.clone(),
            },
        )
        .await
        .unwrap();
    let path = handle::request_path(f.owner.unique_name().unwrap().as_str(), "reconnect").unwrap();
    let mut stream = MessageStream::for_match_rule(
        MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.portal.Request")
            .unwrap()
            .member("Response")
            .unwrap()
            .path(path.clone())
            .unwrap()
            .build(),
        &f.owner,
        Some(4),
    )
    .await
    .unwrap();
    let options = Options::from([(
        "handle_token".into(),
        zbus::zvariant::OwnedValue::from(zbus::zvariant::Str::from("reconnect")),
    )]);
    let reply = f
        .owner
        .call_method(
            Some(f.server.unique_name().unwrap().as_str()),
            OBJECT_PATH,
            Some(INTERFACE),
            "OpenFile",
            &("", "", options),
        )
        .await
        .unwrap();
    assert_eq!(reply.body().deserialize::<OwnedObjectPath>().unwrap(), path);
    response(&mut stream).await;
    f.bridge.registry.lock().await.generation = settings::native::live_generation(&f.bridge.client);
    f.removed(&path).await;
    quiet(&mut stream).await;
}

#[tokio::test]
async fn no_generation_reply_after_reattach_answers_once_and_unexports() {
    let f = Fixture::new().await;
    let (path, mut stream) = f.entry(Phase::Sending).await;
    // choose captured None during reconnect. attach installed the live
    // generation before reply could acquire the registry lock.
    let captured = {
        let mut registry = f.bridge.registry.lock().await;
        registry.generation = None;
        registry.generation
    };
    f.bridge.registry.lock().await.generation = settings::native::live_generation(&f.bridge.client);
    assert!(f.bridge.registry.lock().await.generation.is_some());
    f.bridge.reply(&f.server, captured, 1, None).await;
    response(&mut stream).await;
    f.removed(&path).await;
    f.bridge.reply(&f.server, None, 1, None).await;
    quiet(&mut stream).await;
}

#[tokio::test]
async fn one_slow_list_recovers_and_resets_consecutive_failure_count() {
    let f = Fixture::new().await;
    let (_, mut stream) = f.entry(Phase::Open("inc-1".into())).await;
    f.lists
        .lock()
        .unwrap()
        .extend([Answer::Slow, Answer::Normal, Answer::Refuse, Answer::Normal]);
    for _ in 0..4 {
        f.bridge.poll(&f.server).await;
        quiet(&mut stream).await;
        assert_eq!(
            f.bridge.registry.lock().await.entries[&1].state.phase,
            Phase::Open("inc-1".into())
        );
    }
}

#[tokio::test]
async fn persistent_list_loss_answers_once_and_cancels_live_provider() {
    let mut f = Fixture::new().await;
    let (path, mut stream) = f.entry(Phase::Open("inc-1".into())).await;
    f.lists
        .lock()
        .unwrap()
        .extend([Answer::Refuse, Answer::Refuse]);
    f.bridge.poll(&f.server).await;
    quiet(&mut stream).await;
    f.bridge.poll(&f.server).await;
    response(&mut stream).await;
    f.cancelled().await;
    f.removed(&path).await;
}

#[tokio::test]
async fn missed_owner_signal_is_reconciled_without_response() {
    let mut f = Fixture::new().await;
    let (path, _) = f.entry(Phase::Open("inc-1".into())).await;
    // No attach/watch task is running: NameOwnerChanged is deliberately unseen.
    f.owner.clone().close().await.unwrap();
    f.bridge.poll(&f.server).await;
    f.removed(&path).await;
    f.cancelled().await;
}

#[tokio::test]
async fn cancel_failure_and_timeout_retry_even_after_last_request_ends() {
    for first in [Answer::Refuse, Answer::Silent] {
        let mut f = Fixture::new().await;
        let (path, mut stream) = f.entry(Phase::Open("inc-1".into())).await;
        f.cancels.lock().unwrap().push_back(first);
        f.bridge.close(&f.server, 1).await;
        f.cancelled().await;
        f.removed(&path).await;
        assert_eq!(f.bridge.cancels.lock().unwrap().len(), 1);
        f.bridge.poll(&f.server).await;
        f.cancelled().await;
        assert!(f.bridge.cancels.lock().unwrap().is_empty());
        f.bridge.poll(&f.server).await;
        assert!(f.cancelled.try_recv().is_err());
        quiet(&mut stream).await;
        f.provider.close().await;
    }
}

#[tokio::test]
async fn cancel_retry_budget_is_bounded() {
    let mut f = Fixture::new().await;
    let _entry = f.entry(Phase::Open("inc-1".into())).await;
    f.cancels.lock().unwrap().extend([Answer::Refuse; 4]);
    f.bridge.close(&f.server, 1).await;
    f.cancelled().await;
    for _ in 1..CANCEL_ATTEMPTS {
        f.bridge.poll(&f.server).await;
        f.cancelled().await;
    }
    assert!(f.bridge.cancels.lock().unwrap().is_empty());
    f.bridge.poll(&f.server).await;
    assert!(f.cancelled.try_recv().is_err());
}

#[test]
fn spec_header_describes_the_served_provider_window_and_portal() {
    let spec = include_str!("../../../../docs/docs/spec/files/README.md");
    let header = spec.split("\n\n").nth(1).unwrap();
    assert!(header.contains("DOpus serves the files provider and chooser"));
    assert!(header.contains("window; portald serves FileChooser v4"));
    assert!(!header.contains("Nothing\nhere is served"));
}
