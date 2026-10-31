// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The scene host's own Bus registration.
//!
//! A second Bus client beside the comp port, because the contract's
//! host is Quoin's `shell` service, not `comp`: it answers
//! `shell.scene.*`, publishes `<service>.scene.changed`, and sends scene
//! events to the documents' citizens. It runs on its own worker thread with a
//! current-thread tokio runtime, as comp-service does, and hands requests to the
//! engine over a bounded channel plus the engine's waker. Nothing polls.
//!
//! The name: `shell` when free. When the broker refuses it, the
//! refusal is logged loudly and the host registers under the configured
//! override (`--scene-service NAME` / preference `scene_service`) or not at
//! all. It never silently takes a different name.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use crate::appearance::Look;
use crate::lane::native::{
    Event as SettingsEvent, Lane as SettingsLane, Progress, Session, Ui as SettingsUi,
    Worker as SettingsWorker, bridge,
};
use bus::native_client::BoundedIncomingEvent;
use bus::{ConnState, IncomingCommand, SupervisedClient};
use serde_json::{Value, json};
use tokio::sync::{mpsc as tokio_mpsc, watch};

use crate::command::SceneCommand;

/// The contract's host name.
pub const DEFAULT_SERVICE: &str = "shell";
/// The broker's registry topic (comp-model `REGISTRY_TOPIC`).
pub const REGISTRY_TOPIC: &str = "noded.props.changed";
/// Requests the engine has not taken yet; past this the worker answers
/// rc 11 `QUEUE_FULL` itself.
pub const INBOUND_CAPACITY: usize = 64;
/// Scene events in flight to citizens (contract: at most 128 pending).
pub const MAX_PENDING_EVENTS: usize = 128;
/// A citizen's handler answers within this, or the event is abandoned.
pub const EVENT_TIMEOUT: Duration = Duration::from_secs(2);
const SEND_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_INITIAL: Duration = Duration::from_secs(1);
/// A registry burst collapses into one follow-up read after this pause.
const REGISTRY_FOLLOW_UP: Duration = Duration::from_millis(100);
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(2);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(2300);
const RUNTIME_GRACE: Duration = Duration::from_millis(100);
const DEREGISTER_BUDGET: Duration = Duration::from_millis(200);

/// Wakes the engine's loop; called once per delivered message.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// How the host registers and where the broker is.
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// The name to fall back to when `shell` is refused; `None` refuses.
    pub service_override: Option<String>,
    /// The comp port's URL resolution (`comp_service::default_noded_url()`:
    /// `MIXOS_NODED_URL`, else the node config, else the loopback broker).
    pub noded_url: String,
}

/// One admitted scene request.
#[derive(Clone, Debug)]
pub struct Request {
    pub kind: SceneCommand,
    pub from: String,
    pub command: String,
    pub id: Option<String>,
    pub body: String,
    pub headers: BTreeMap<String, String>,
    /// The broker connection it was admitted on
    /// (`SupervisedClient::connection_generation`). A request from an older
    /// connection is refused unapplied, and no reply crosses a reconnect.
    pub generation: u64,
}

/// What the worker hands the engine.
#[derive(Debug)]
pub enum Inbound {
    /// Registered (or re-registered after the first) under this name.
    Registered(String),
    /// Initial registration was refused, or the established connection ended
    /// terminally. Scene RPCs are unavailable; settings resources remain active.
    Refused(String),
    Request(Request),
    /// The broker's full set of registered services from a fresh snapshot.
    Live(BTreeSet<String>),
    Titlebar {
        generation: u64,
        service: String,
        form: Value,
    },
    Repaint,
    /// settingsd's answer to a [`Port::apply_settings`]: its reply body, or
    /// a refusal body (`error_code` `SETTINGS_CONFLICT`, `SETTINGS_REFUSED`
    /// or `SETTINGS_UNAVAILABLE`).
    Settings {
        operation: String,
        outcome: Result<Value, Value>,
    },
}

enum Outbound {
    Titlebars(Vec<String>),
    WakeAfter(Duration),
    /// `settings.apply` to settingsd, answered as [`Inbound::Settings`].
    Settings {
        operation: String,
        body: Value,
    },
    Reply {
        to: String,
        command: String,
        id: Option<String>,
        rc: u8,
        body: String,
        generation: u64,
    },
    /// A topic wire: `<service>.scene.changed`, or with `panel`
    /// `<service>.panel.changed`, retained as Quoin's is (a loader that
    /// subscribes late still reads the current panels).
    Publish {
        panel: bool,
        wire: String,
    },
}

struct Event {
    to: String,
    kind: String,
    body: Value,
}

struct PendingEvent(Arc<AtomicUsize>);
impl Drop for PendingEvent {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The engine's end of the port.
pub struct Port {
    inbound: Receiver<Inbound>,
    outbound: tokio_mpsc::UnboundedSender<Outbound>,
    sink: EventSink,
    shutdown: watch::Sender<bool>,
    completion: Receiver<()>,
    thread: Option<thread::JoinHandle<()>>,
    /// The registered client, once there is one: the engine reads the live
    /// connection generation from it directly (an atomic load), so a
    /// reconnect the worker has not yet reported still fences.
    client: Arc<OnceLock<Arc<SupervisedClient>>>,
    settings_ui: Option<SettingsUi<Arc<Look>>>,
    registry: RegistryMailbox,
    titlebars: Vec<String>,
}

const REGISTRY_HISTORY: usize = 64;

#[derive(Clone, Default)]
struct RegistryMailbox(Arc<Mutex<RegistryState>>);

#[derive(Default)]
struct RegistryState {
    pending: Option<(u64, BTreeSet<String>)>,
    events: VecDeque<String>,
    sequence: u64,
}

impl RegistryState {
    fn record(&mut self, event: String) {
        self.sequence = self.sequence.wrapping_add(1);
        let event: String = event.chars().take(512).collect();
        tracing::debug!(event = %event, "scene host registry");
        if self.events.len() == REGISTRY_HISTORY {
            self.events.pop_front();
        }
        self.events.push_back(format!("{}: {event}", self.sequence));
    }
}

impl RegistryMailbox {
    fn record(&self, event: impl Into<String>) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record(event.into());
    }

    fn store(&self, generation: u64, services: BTreeSet<String>) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.record(format!(
            "mailbox written generation={generation} count={} prefs={}",
            services.len(),
            services.contains("prefs")
        ));
        state.pending = Some((generation, services));
    }

    #[cfg(test)]
    fn history(&self) -> String {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .events
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Coalesce invalidations without cancelling a read that is making progress.
/// A burst during one read owes one follow-up read, regardless of its size.
struct RegistryRead<F> {
    pending: Option<Pin<Box<F>>>,
    dirty: bool,
}

impl<F: std::future::Future> RegistryRead<F> {
    fn new() -> Self {
        Self {
            pending: None,
            dirty: false,
        }
    }

    fn request(&mut self, start: impl FnOnce() -> F) {
        if self.pending.is_some() {
            self.dirty = true;
        } else {
            self.pending = Some(Box::pin(start()));
        }
    }

    /// Release the completed future and return whether it owes another read.
    fn complete(&mut self) -> bool {
        self.pending = None;
        std::mem::take(&mut self.dirty)
    }

    /// Only a connection change or terminal shutdown cancels live work.
    fn cancel(&mut self) {
        self.pending = None;
        self.dirty = false;
    }
}

fn take_registry(registry: &RegistryMailbox, live: Option<u64>) -> Option<Inbound> {
    // A temporary lack of live evidence must not consume the only baseline.
    // Once a new connection is live, its snapshot supersedes any old one.
    let mut state = registry
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.pending.as_ref()?;
    let Some(generation) = live else {
        state.record("mailbox retained: no live generation on UI".into());
        return None;
    };
    let (captured, services) = state.pending.take()?;
    if captured != generation {
        let event = format!("mailbox dropped: captured generation={captured} live={generation}");
        tracing::warn!(%event, "scene host registry generation mismatch");
        state.record(event);
        return None;
    }
    state.record(format!(
        "try_recv -> Inbound::Live generation={generation} count={} prefs={}",
        services.len(),
        services.contains("prefs")
    ));
    Some(Inbound::Live(services))
}

/// The names to try, in order: `shell`, then the override when it differs.
pub fn candidate_names(service_override: Option<&str>) -> Vec<String> {
    let mut names = vec![DEFAULT_SERVICE.to_owned()];
    if let Some(name) = service_override
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != DEFAULT_SERVICE)
    {
        names.push(name.to_owned());
    }
    names
}

impl Port {
    /// Start the worker. The broker connection is the worker's: a missing
    /// noded is retried with backoff and never blocks the compositor.
    pub fn start(config: HostConfig, waker: Waker) -> Result<Self, String> {
        Self::start_at(
            config,
            waker,
            config::path(config::Dir::Var).join("compd/cache/settings"),
        )
    }

    fn start_at(
        config: HostConfig,
        waker: Waker,
        cache_directory: std::path::PathBuf,
    ) -> Result<Self, String> {
        let binding =
            settings::session::binding().map_err(|error| format!("settings session: {error:?}"))?;
        let consumer = settings::consumer::Consumer::for_shell(binding)
            .map_err(|error| format!("shell settings: {error:?}"))?;
        let settings_worker =
            SettingsWorker::offline_with_cache(cache_directory, |snapshot: &settings::Snapshot| {
                crate::appearance::build(snapshot).map(Arc::new)
            });
        let (ui, lane) = bridge(Session::new(consumer), settings_worker);
        let registry = RegistryMailbox::default();
        let worker_registry = registry.clone();
        let (inbound_tx, inbound) = mpsc::sync_channel(INBOUND_CAPACITY);
        let (outbound, outbound_rx) = tokio_mpsc::unbounded_channel();
        let (events, events_rx) = tokio_mpsc::unbounded_channel();
        let (shutdown, shutdown_rx) = watch::channel(false);
        let (completion_tx, completion) = mpsc::sync_channel(1);
        let pending_events = Arc::new(AtomicUsize::new(0));
        let pending = Arc::clone(&pending_events);
        let client: Arc<OnceLock<Arc<SupervisedClient>>> = Arc::new(OnceLock::new());
        let published = Arc::clone(&client);
        let thread = thread::Builder::new()
            .name("compd-scenes".into())
            .spawn(move || {
                let _completion = CompletionOnDrop(completion_tx);
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::error!(%error, "scene host: failed to build its Bus runtime");
                        return;
                    }
                };
                let delivery = Delivery {
                    sender: inbound_tx,
                    waker,
                };
                runtime.block_on(worker(
                    config,
                    delivery,
                    outbound_rx,
                    events_rx,
                    pending,
                    shutdown_rx,
                    published,
                    lane,
                    worker_registry,
                ));
                runtime.shutdown_timeout(RUNTIME_GRACE);
            })
            .map_err(|error| format!("failed to spawn the scene host's Bus worker: {error}"))?;
        let sink = EventSink {
            events,
            pending: pending_events,
        };
        Ok(Self {
            inbound,
            outbound,
            sink,
            shutdown,
            completion,
            thread: Some(thread),
            client,
            settings_ui: Some(ui),
            registry,
            titlebars: Vec::new(),
        })
    }

    pub(crate) fn take_settings_ui(&mut self) -> SettingsUi<Arc<Look>> {
        self.settings_ui
            .take()
            .expect("settings UI taken only once")
    }

    pub fn titlebar_services(&mut self, services: Vec<String>) {
        if self.titlebars != services {
            self.titlebars = services.clone();
            let _ = self.outbound.send(Outbound::Titlebars(services));
        }
    }

    pub fn wake_after(&self, delay: Duration) {
        let _ = self.outbound.send(Outbound::WakeAfter(delay));
    }
    pub(crate) fn settings_generation(&self) -> Option<u64> {
        self.client
            .get()
            .and_then(|client| settings::native::live_generation(client))
    }

    /// The live broker connection's generation; `None` before registering.
    pub fn connection_generation(&self) -> Option<u64> {
        self.client
            .get()
            .map(|client| client.connection_generation())
    }

    /// The next message from the worker, without blocking.
    pub fn try_recv(&self) -> Option<Inbound> {
        self.inbound
            .try_recv()
            .ok()
            .or_else(|| take_registry(&self.registry, self.settings_generation()))
    }

    pub fn reply(&self, request: &Request, rc: u8, body: String) {
        let _ = self.outbound.send(Outbound::Reply {
            generation: request.generation,
            to: request.from.clone(),
            command: request.command.clone(),
            id: request.id.clone(),
            rc,
            body,
        });
    }

    /// Publish one `<service>.scene.changed` wire, in call order.
    pub fn publish(&self, wire: String) {
        let _ = self.outbound.send(Outbound::Publish { panel: false, wire });
    }

    /// Publish one `<service>.panel.changed` wire, retained.
    pub fn publish_panel(&self, wire: String) {
        let _ = self.outbound.send(Outbound::Publish { panel: true, wire });
    }

    /// `settings.apply` with `body` to settingsd, for a Bus caller waiting on
    /// `operation`: the outcome returns as [`Inbound::Settings`]. False when
    /// the worker is gone.
    pub(crate) fn apply_settings(&self, operation: &str, body: Value) -> bool {
        self.outbound
            .send(Outbound::Settings {
                operation: operation.to_owned(),
                body,
            })
            .is_ok()
    }

    /// A scene event: a directed request to `citizen`, command = the handler
    /// name, body `{scene,node,kind,value?,item?}`. False when the
    /// contract's 128 pending events are already in flight (dropped, logged).
    pub fn emit(&self, citizen: &str, kind: &str, body: Value) -> bool {
        self.sink.emit(citizen, kind, body)
    }

    /// A sender for scene events that any thread may hold (the UI
    /// surfaces' message handlers).
    pub fn event_sink(&self) -> EventSink {
        self.sink.clone()
    }

    /// Deregister and stop, bounded.
    pub fn finish(mut self) {
        let _ = self.shutdown.send(true);
        let Some(thread) = self.thread.take() else {
            return;
        };
        match self.completion.recv_timeout(SHUTDOWN_GRACE) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if thread.join().is_err() {
                    tracing::error!("scene host Bus worker panicked during shutdown");
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                tracing::warn!("scene host Bus worker did not stop in time and was detached");
            }
        }
    }
}

/// Sends scene events through the worker, bounded at
/// [`MAX_PENDING_EVENTS`] in flight.
#[derive(Clone)]
pub struct EventSink {
    events: tokio_mpsc::UnboundedSender<Event>,
    pending: Arc<AtomicUsize>,
}

impl EventSink {
    /// False when 128 events are already pending (dropped, logged) or the
    /// worker is gone.
    pub fn emit(&self, citizen: &str, kind: &str, body: Value) -> bool {
        if self.pending.fetch_add(1, Ordering::AcqRel) >= MAX_PENDING_EVENTS {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            tracing::warn!(
                citizen,
                kind,
                "scene event dropped: {MAX_PENDING_EVENTS} events already pending"
            );
            return false;
        }
        let event = Event {
            to: citizen.to_owned(),
            kind: kind.to_owned(),
            body,
        };
        if self.events.send(event).is_err() {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        true
    }
}

struct CompletionOnDrop(SyncSender<()>);

impl Drop for CompletionOnDrop {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

#[derive(Clone)]
struct Delivery {
    sender: SyncSender<Inbound>,
    waker: Waker,
}

impl Delivery {
    /// False when the engine's queue is full or gone (the message is
    /// dropped; every caller answers or logs that itself).
    fn send(&self, message: Inbound) -> bool {
        let sent = self.sender.try_send(message).is_ok();
        if sent {
            (self.waker)();
        }
        sent
    }
}

/// Where one incoming frame goes.
#[derive(Debug, PartialEq)]
pub enum Route {
    Describe,
    Scene(SceneCommand),
    /// A verified registry invalidation; only a fresh read supplies names.
    Live,
    /// A registry claim not from the local broker; dropped, logged.
    Forged,
    /// A request for a command this host does not serve.
    Unknown,
    /// A topic delivery or a reply: nothing to answer.
    Ignore(&'static str),
}

/// Route one incoming frame for the host registered as `service`.
pub fn route(service: &str, command: &IncomingCommand) -> Route {
    // Production topic publications may omit `from`: the publisher identity
    // is stamped by noded as broker_service (as on the main comp Bus port).
    if command.topic() == Some(REGISTRY_TOPIC) {
        if command.from != "noded" && command.header("broker_service") != Some("noded") {
            return Route::Ignore("missing_broker_identity");
        }
        if !from_local_broker(command) {
            tracing::warn!(
                from = %command.from,
                path = ?command.header("path"),
                broker_service = ?command.header("broker_service"),
                broker_origin = ?command.header("broker_origin"),
                reason = "non_local_broker_origin",
                "scene host registry Route::Forged"
            );
            return Route::Forged;
        }
        // noded supplies a path header. Keep the body fallback for older
        // publishers, but never require a list-valued diff: services.count
        // and every other services.* change invalidate the same registry.
        let body;
        let path = if let Some(path) = command.header("path") {
            path
        } else {
            body = match serde_json::from_str::<Value>(&command.body) {
                Ok(body) => body,
                Err(_) => return Route::Ignore("invalid_payload_json"),
            };
            match body.get("path").and_then(Value::as_str) {
                Some(path) => path,
                None => return Route::Ignore("missing_or_invalid_path"),
            }
        };
        if !path.starts_with("services.") {
            return Route::Ignore("unrelated_property_path");
        }
        tracing::debug!(
            path,
            generation = command.generation,
            from = %command.from,
            broker_service = ?command.header("broker_service"),
            "scene host registry Route::Live"
        );
        return Route::Live;
    }
    if command.is_topic_delivery() {
        return Route::Ignore("unrelated_topic");
    }
    if command.command.is_empty() {
        return Route::Ignore("empty_command");
    }
    if matches!(command.command.as_str(), "HELP" | "app.describe") {
        return Route::Describe;
    }
    match SceneCommand::parse(service, &command.command) {
        Some(kind) => Route::Scene(kind),
        None if command.id.is_some() => Route::Unknown,
        None => Route::Ignore("unknown_command_without_id"),
    }
}

/// Only the local broker speaks as `noded`.
fn from_local_broker(command: &IncomingCommand) -> bool {
    command
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("broker_origin"))
        .all(|(_, origin)| origin.eq_ignore_ascii_case("local"))
}

fn titlebar_change<'a>(command: &'a IncomingCommand, services: &[String]) -> Option<&'a str> {
    let service = command.topic()?.strip_suffix(".ui.titlebar.changed")?;
    (services.iter().any(|name| name == service)
        && command.header("broker_service") == Some(service)
        && command.header("broker_origin") == Some("local")
        && from_local_broker(command))
    .then_some(service)
}

async fn titlebar_read(
    client: Arc<SupervisedClient>,
    service: String,
    epoch: u64,
    subscribe: bool,
    diagnostics: RegistryMailbox,
) -> (u64, u64, String, Value, bool) {
    let generation = client.connection_generation();
    diagnostics.record(format!(
        "titlebar read service={service} epoch={epoch} generation={generation} subscribe={subscribe}"
    ));
    let subscribed = if subscribe {
        let result = tokio::time::timeout(
            SEND_TIMEOUT,
            client.subscribe_topic(&format!("{service}.ui.titlebar.changed")),
        )
        .await;
        diagnostics.record(format!(
            "titlebar subscribe service={service} result={result:?}"
        ));
        matches!(result, Ok(Ok(())))
    } else {
        true
    };
    let form = if subscribed {
        match tokio::time::timeout(
            EVENT_TIMEOUT,
            client.call(&service, &format!("{service}.ui.titlebar.form"), json!({})),
        )
        .await
        {
            Ok(Ok(body)) => {
                diagnostics.record(format!(
                    "titlebar form service={service} hosted={} revision={} items={}",
                    body["hosted"],
                    body["bar"]["revision"],
                    body["bar"]["items"].as_array().map_or(0, Vec::len)
                ));
                body
            }
            result => {
                diagnostics.record(format!("titlebar form service={service} failed={result:?}"));
                Value::Null
            }
        }
    } else {
        Value::Null
    };
    (epoch, generation, service, form, subscribed)
}

enum Connected {
    Client(Arc<SupervisedClient>, String),
    Refused(String),
    Stopped,
}

async fn connect(config: HostConfig, mut shutdown: watch::Receiver<bool>) -> Connected {
    let names = candidate_names(config.service_override.as_deref());
    let mut refusals = Vec::new();
    for name in &names {
        let client = Arc::new(
            SupervisedClient::connect_options(name, &config.noded_url)
                .fatal_on_registration_rejection(true)
                .bounded_incoming(INBOUND_CAPACITY)
                .start(),
        );
        let mut lifecycle = client.subscribe_state();
        loop {
            // A Connected edge may have coalesced with subsequent loss or
            // rejection. Once established, this identity must never fall back.
            if client.connection_generation() > 0 {
                return Connected::Client(client, name.clone());
            }
            let state = *lifecycle.borrow_and_update();
            if matches!(state, ConnState::Fatal | ConnState::ShuttingDown) {
                // Registration may have completed between the first generation
                // sample and this terminal state read.
                if client.connection_generation() > 0 {
                    return Connected::Client(client, name.clone());
                }
                let refusal = client
                    .registration_rejection()
                    .map(|refusal| format!("rc {}: {}", refusal.rc, refusal.message));
                if tokio::time::timeout(DEREGISTER_BUDGET, client.close())
                    .await
                    .is_err()
                {
                    return Connected::Refused(format!(
                        "{name}: rejected supervisor did not retire; no fallback attempted"
                    ));
                }
                let Some(refusal) = refusal else {
                    return Connected::Refused(format!(
                        "{name}: supervisor stopped without an initial registration refusal"
                    ));
                };
                tracing::error!("SCENE HOST: the broker refused the Bus name `{name}` ({refusal})");
                refusals.push(format!("{name}: {refusal}"));
                break;
            }
            tokio::select! {
                changed = lifecycle.changed() => {
                    if changed.is_err() { return Connected::Refused(format!("{name}: supervisor lifecycle closed")); }
                }
                _ = shutdown.changed() => return Connected::Stopped,
            }
        }
        if let Some(next) = names
            .iter()
            .skip_while(|candidate| *candidate != name)
            .nth(1)
        {
            tracing::error!("SCENE HOST: falling back to the configured override name `{next}`");
        }
    }
    let reason = if names.len() == 1 {
        format!(
            "{}; no --scene-service / scene_service override is configured, so the scene host is OFF",
            refusals.join("; ")
        )
    } else {
        format!("{}; the scene host is OFF", refusals.join("; "))
    };
    tracing::error!("SCENE HOST: {reason}");
    Connected::Refused(reason)
}

#[allow(clippy::too_many_arguments)]
async fn worker(
    config: HostConfig,
    delivery: Delivery,
    mut outbound: tokio_mpsc::UnboundedReceiver<Outbound>,
    mut events: tokio_mpsc::UnboundedReceiver<Event>,
    pending_events: Arc<AtomicUsize>,
    mut shutdown: watch::Receiver<bool>,
    published: Arc<OnceLock<Arc<SupervisedClient>>>,
    mut lane: SettingsLane<Arc<Look>>,
    registry_mailbox: RegistryMailbox,
) {
    registry_mailbox.record("worker started; connecting scene host");
    let waker = Arc::clone(&delivery.waker);
    let notify = move |needed| {
        if needed {
            waker();
        }
    };
    let mut connecting = Some(Box::pin(connect(config.clone(), shutdown.clone())));
    // The existing connection attempt must not stop the resource worker. A
    // refused service still receives offline presentation work until shutdown.
    let connected = loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break None; }
            }
            progress = lane.drive() => {
                match progress {
                    Progress::Wake => notify(true),
                    Progress::UiClosed => break None,
                    Progress::Updated => {}
                }
            }
            result = async { connecting.as_mut().expect("guarded connection").await }, if connecting.is_some() => {
                connecting = None;
                match result {
                    Connected::Client(client, service) => break Some((client, service)),
                    Connected::Refused(reason) => {
                        registry_mailbox.record(format!("connection refused: {reason}"));
                        let _ = delivery.send(Inbound::Refused(reason));
                    }
                    Connected::Stopped => {
                        registry_mailbox.record("connection stopped");
                        break None;
                    }
                }
            }
        }
    };
    // Cancellation drops the unpublished candidate and signals its supervisor
    // before the common cache drain and bounded runtime shutdown.
    drop(connecting.take());
    let deadline = if let Some((client, service)) = &connected {
        let _ = published.set(Arc::clone(client));
        registry_mailbox.record(format!(
            "connected service={service} state={:?} generation={}",
            client.state(),
            client.connection_generation()
        ));
        notify(lane.connect(Arc::clone(client)));
        tracing::info!(
            "scene host: registered as `{service}` via {}",
            config.noded_url
        );
        if !delivery.send(Inbound::Registered(service.clone())) {
            tracing::warn!("scene host: the engine's queue refused the registration notice");
        }
        serve(
            client,
            service,
            &delivery,
            &mut outbound,
            &mut events,
            pending_events,
            &mut shutdown,
            &mut lane,
            &registry_mailbox,
            &notify,
        )
        .await
    } else {
        std::time::Instant::now() + SHUTDOWN_BUDGET
    };
    if let Err(error) = lane.flush_cache(deadline).await {
        tracing::warn!(?error, "scene host settings cache drain failed");
    }
    if let Some((client, _)) = connected {
        let close_deadline = tokio::time::Instant::from_std(deadline);
        let deregister_deadline =
            close_deadline.min(tokio::time::Instant::now() + DEREGISTER_BUDGET);
        match tokio::time::timeout_at(deregister_deadline, client.deregister()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::debug!(%error, "scene host deregister did not complete cleanly")
            }
            Err(_) => tracing::warn!("scene host deregister timed out"),
        }
        if tokio::time::timeout_at(close_deadline, client.close())
            .await
            .is_err()
        {
            tracing::warn!("scene host Bus close exceeded shutdown budget");
        }
    }
    notify(lane.publish(SettingsEvent::Wake));
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    client: &Arc<SupervisedClient>,
    service: &str,
    delivery: &Delivery,
    outbound: &mut tokio_mpsc::UnboundedReceiver<Outbound>,
    events: &mut tokio_mpsc::UnboundedReceiver<Event>,
    pending_events: Arc<AtomicUsize>,
    shutdown: &mut watch::Receiver<bool>,
    lane: &mut SettingsLane<Arc<Look>>,
    registry_mailbox: &RegistryMailbox,
    notify: &impl Fn(bool),
) -> std::time::Instant {
    let Some(mut incoming) = client.incoming_bounded() else {
        tracing::error!("scene host: the Bus client has no incoming lane; the host is OFF");
        let _ = delivery.send(Inbound::Refused("no incoming lane".into()));
        return std::time::Instant::now() + SHUTDOWN_BUDGET;
    };
    let mut lifecycle = client.subscribe_state();
    let mut lifecycle_open = true;
    let mut incoming_open = true;
    let initial_state = *lifecycle.borrow_and_update();
    registry_mailbox.record(format!(
        "serve started state={initial_state:?} generation={}",
        client.connection_generation()
    ));
    if matches!(initial_state, ConnState::Fatal | ConnState::ShuttingDown) {
        let _ = delivery.send(Inbound::Refused(terminal_reason(client)));
    }
    let topics = (
        format!("{service}.scene.changed"),
        format!("{service}.panel.changed"),
    );
    let mut flights = tokio::task::JoinSet::new();
    let mut replies = tokio::task::JoinSet::new();
    let mut sends = tokio::task::JoinSet::new();
    let mut titlebars = Vec::<String>::new();
    let mut bars = tokio::task::JoinSet::new();
    let mut pending_titlebars = BTreeSet::new();
    let mut dirty_titlebars = BTreeSet::new();
    let mut subscribed_titlebars = BTreeSet::new();
    let mut titlebar_epoch = 0_u64;
    // Recover lost invalidations and failed initial reads at a slow resync rate.
    let mut titlebar_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(30),
        Duration::from_secs(30),
    );
    titlebar_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut registry = RegistryRead::new();
    let mut registry_retries = 0;
    let mut registry_subscription = (initial_state == ConnState::Connected).then(|| {
        Box::pin(registry_subscribe(
            Arc::clone(client),
            registry_mailbox.clone(),
        ))
    });
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            progress = lane.drive() => {
                match progress {
                    Progress::Wake => notify(true),
                    Progress::UiClosed => break,
                    Progress::Updated => {}
                }
            }
            changed = lifecycle.changed(), if lifecycle_open => {
                if changed.is_err() { lifecycle_open = false; }
                let state = *lifecycle.borrow_and_update();
                registry_mailbox.record(format!("lifecycle state={state:?} generation={}", client.connection_generation()));
                notify(lane.publish(SettingsEvent::Wake));
                registry.cancel();
                bars.abort_all();
                pending_titlebars.clear();
                subscribed_titlebars.clear();
                dirty_titlebars = titlebars.iter().cloned().collect();
                titlebar_tick.reset();
                titlebar_epoch = titlebar_epoch.wrapping_add(1);
                registry_subscription = (lifecycle_open && state == ConnState::Connected)
                    .then(|| Box::pin(registry_subscribe(Arc::clone(client), registry_mailbox.clone())));
                if !lifecycle_open || matches!(state, ConnState::Fatal | ConnState::ShuttingDown) {
                    let _ = delivery.send(Inbound::Refused(terminal_reason(client)));
                }
                registry_retries = 0;
            }
            subscribed = async { registry_subscription.as_mut().expect("guarded registry subscription").await }, if registry_subscription.is_some() => {
                registry_subscription = None;
                if subscribed {
                    // Subscribe before read so a departure in the gap cannot
                    // leave a successful but obsolete full-set baseline.
                    registry.request(|| registry_read(Arc::clone(client), REGISTRY_FOLLOW_UP, registry_mailbox.clone()));
                    registry_retries = 0;
                }
            }
            result = async { registry.pending.as_mut().expect("guarded registry read").await }, if registry.pending.is_some() => {
                let dirty = registry.complete();
                registry_mailbox.record(format!("read completed dirty={dirty} success={}", result.is_some()));
                if let Some((generation, services)) = result {
                    if settings::native::live_generation(client) == Some(generation) {
                        registry_mailbox.store(generation, services);
                        (delivery.waker)();
                        registry_mailbox.record("mailbox wake fired");
                        registry_retries = 0;
                        if dirty {
                            registry_mailbox.record("dirty follow-up read scheduled");
                            registry.request(|| registry_read(Arc::clone(client), REGISTRY_FOLLOW_UP, registry_mailbox.clone()));
                        }
                    } else {
                        let live = settings::native::live_generation(client);
                        registry_mailbox.record(format!("read result dropped: generation={generation} live={live:?}"));
                        tracing::warn!(generation, ?live, "scene host registry read result generation mismatch");
                    }
                } else if settings::native::live_generation(client).is_some() {
                    let delay = RETRY_INITIAL * (1 << registry_retries.min(3));
                    registry_retries = (registry_retries + 1).min(3);
                    registry_mailbox.record(format!("read retry scheduled delay={delay:?}"));
                    registry.request(|| registry_read(Arc::clone(client), delay, registry_mailbox.clone()));
                } else {
                    tracing::debug!("scene host: registry recovery awaits a live connection");
                }
            }
            command = incoming.recv(), if incoming_open && replies.len() < INBOUND_CAPACITY => {
                match command {
                    Some(BoundedIncomingEvent::Command(command)) => {
                        if command.topic().is_some_and(|topic| topic.ends_with(".ui.titlebar.changed")) {
                            let service = titlebar_change(&command, &titlebars);
                            let live = settings::native::live_generation(client);
                            registry_mailbox.record(format!(
                                "titlebar change topic={:?} owner={:?} origin={:?} accepted={} generation={} live={live:?}",
                                command.topic(), command.header("broker_service"), command.header("broker_origin"),
                                service.is_some() && live == Some(command.generation), command.generation
                            ));
                            if let Some(service) = service && live == Some(command.generation) {
                                dirty_titlebars.insert(service.to_owned());
                            }
                        } else if let Some(wake) = lane.delivery(&command) {
                            notify(wake);
                        } else {
                            let routed = route(service, &command);
                            if command.topic() == Some(REGISTRY_TOPIC) {
                                let (kind, reason) = match &routed {
                                    Route::Live => ("Live", "verified_broker_services_change"),
                                    Route::Forged => ("Forged", "non_local_broker_origin"),
                                    Route::Ignore(reason) => ("Ignore", *reason),
                                    _ => ("Ignore", "not_registry_change"),
                                };
                                registry_mailbox.record(format!(
                                    "Route::{kind} topic={REGISTRY_TOPIC} path={:?} command={:?} from={:?} broker_service={:?} broker_origin={:?} generation={} reason={reason}",
                                    command.header("path"), command.command, command.from, command.header("broker_service"), command.header("broker_origin"), command.generation
                                ));
                            }
                            if let Route::Live = routed {
                                let live = settings::native::live_generation(client);
                                if live == Some(command.generation) {
                                    // A buffered diff can predate a just-completed
                                    // baseline read. Treat it as an invalidation,
                                    // never overwrite a newer snapshot with its set.
                                    registry_mailbox.record(format!("registry invalidated in_flight={}", registry.pending.is_some()));
                                    registry.request(|| registry_read(Arc::clone(client), REGISTRY_FOLLOW_UP, registry_mailbox.clone()));
                                } else {
                                    registry_mailbox.record(format!("Route::Live dropped: generation={} live={live:?}", command.generation));
                                    tracing::warn!(generation = command.generation, ?live, "scene host registry delivery generation mismatch");
                                }
                            } else if let Some((command, rc, body)) = admit(service, delivery, command) {
                                let client = Arc::clone(client);
                                replies.spawn(async move { refuse(&client, &command, rc, body).await; });
                            }
                        }
                    }
                    Some(BoundedIncomingEvent::Overflow { dropped }) => {
                        dirty_titlebars.extend(titlebars.iter().cloned());
                        registry_mailbox.record(format!("incoming overflow dropped={dropped}"));
                        notify(lane.publish(SettingsEvent::Lost));
                        registry.request(|| registry_read(Arc::clone(client), REGISTRY_FOLLOW_UP, registry_mailbox.clone()));
                        tracing::warn!("scene host: incoming queue overflow; recovering settings and registry");
                    }
                    None => {
                        incoming_open = false;
                        registry.cancel();
                        registry_subscription = None;
                        registry_mailbox.record("incoming lane closed");
                        notify(lane.publish(SettingsEvent::Wake));
                        let _ = delivery.send(Inbound::Refused("Bus incoming lane closed; retaining settings resources".into()));
                    }
                }
            }
            Some(message) = outbound.recv(), if sends.is_empty() => {
                match message {
                    Outbound::Titlebars(services) => {
                        registry_mailbox.record(format!("titlebar targets={services:?}"));
                        if titlebars.is_empty() {
                            titlebar_tick.reset();
                        }
                        dirty_titlebars.extend(pending_titlebars.iter().cloned());
                        dirty_titlebars.extend(
                            services.iter().filter(|name| !titlebars.contains(name)).cloned()
                        );
                        dirty_titlebars.retain(|name| services.contains(name));
                        bars.abort_all();
                        pending_titlebars.clear();
                        titlebar_epoch = titlebar_epoch.wrapping_add(1);
                        // Keep dormant subscriptions; an old unsubscribe must
                        // not race a returning window's new subscription.
                        titlebars = services;
                    }
                    Outbound::WakeAfter(delay) => {
                        let delivery = delivery.clone();
                        flights.spawn(async move { tokio::time::sleep(delay).await; delivery.send(Inbound::Repaint); });
                    }
                    Outbound::Settings { operation, body } => {
                        let (client, delivery) = (Arc::clone(client), delivery.clone());
                        flights.spawn(async move {
                            let outcome = settings_apply(&client, &body).await;
                            if !delivery.send(Inbound::Settings { operation, outcome }) {
                                tracing::warn!("scene host: settings.apply outcome dropped: the engine queue is full");
                            }
                        });
                    }
                    message => {
                        let (client, topics) = (Arc::clone(client), topics.clone());
                        sends.spawn(async move { send(&client, &topics, message).await; });
                    }
                }
            }
            _ = titlebar_tick.tick(), if !titlebars.is_empty() && settings::native::live_generation(client).is_some() => {
                dirty_titlebars.extend(titlebars.iter().cloned());
            }
            _ = std::future::ready(()), if dirty_titlebars.iter().any(|service| !pending_titlebars.contains(service)) && settings::native::live_generation(client).is_some() => {
                for service in dirty_titlebars.iter().filter(|service| !pending_titlebars.contains(*service)).cloned().collect::<Vec<_>>() {
                    dirty_titlebars.remove(&service);
                    pending_titlebars.insert(service.clone());
                    let subscribe = !subscribed_titlebars.contains(&service);
                    bars.spawn(titlebar_read(Arc::clone(client), service, titlebar_epoch, subscribe, registry_mailbox.clone()));
                }
            }
            Some(result) = bars.join_next(), if !bars.is_empty() => {
                if let Ok((epoch, generation, service, form, subscribed)) = result && epoch == titlebar_epoch {
                    pending_titlebars.remove(&service);
                    if titlebars.contains(&service) && settings::native::live_generation(client) == Some(generation) {
                        if subscribed {
                            subscribed_titlebars.insert(service.clone());
                        }
                        let _ = delivery.send(Inbound::Titlebar { generation, service, form });
                    }
                }
            }
            Some(event) = events.recv() => {
                let client = Arc::clone(client);
                let pending = Arc::clone(&pending_events);
                let pending = PendingEvent(pending);
                flights.spawn(async move {
                    let _pending = pending;
                    let call = client.call(&event.to, &event.kind, event.body);
                    match tokio::time::timeout(EVENT_TIMEOUT, call).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(error)) if event.kind.ends_with(".ui.menu.popup") || event.kind.ends_with(".execute") => tracing::warn!(%error, to = %event.to, "window chrome call failed"),
                        Err(_) if event.kind.ends_with(".ui.menu.popup") || event.kind.ends_with(".execute") => tracing::warn!(to = %event.to, "window chrome call timed out"),
                        Ok(Err(error)) if event.kind == "settings.apply" => tracing::warn!(%error, "corner panel mode not saved"),
                        Err(_) if event.kind == "settings.apply" => tracing::warn!("corner panel mode save timed out"),
                        Ok(Err(error)) => tracing::debug!(%error, to = %event.to, kind = %event.kind, "scene event failed"),
                        Err(_) => tracing::debug!(to = %event.to, kind = %event.kind, "scene event timed out"),
                    }
                });
            }
            Some(_) = flights.join_next(), if !flights.is_empty() => {}
            Some(_) = replies.join_next(), if !replies.is_empty() => {}
            Some(_) = sends.join_next(), if !sends.is_empty() => {}
        }
    }
    let deadline = std::time::Instant::now() + SHUTDOWN_BUDGET;
    notify(lane.publish(SettingsEvent::Wake));
    // Finish the earlier active send before later queued messages. A bounded
    // drain that expires cancels the remaining sequence, preserving its order.
    if tokio::time::timeout(Duration::from_millis(50), async {
        while sends.join_next().await.is_some() {}
        while let Ok(message) = outbound.try_recv() {
            send(client, &topics, message).await;
        }
    })
    .await
    .is_err()
    {
        tracing::warn!("scene host ordered outbound drain timed out");
    }
    flights.abort_all();
    bars.abort_all();
    replies.abort_all();
    sends.abort_all();
    deadline
}

fn terminal_reason(client: &SupervisedClient) -> String {
    client.registration_rejection().map_or_else(
        || "Bus supervisor stopped; retaining settings resources".into(),
        |refusal| {
            format!(
                "registration refused (rc {}): {}; retaining settings resources",
                refusal.rc, refusal.message
            )
        },
    )
}

async fn registry_read(
    client: Arc<SupervisedClient>,
    delay: Duration,
    diagnostics: RegistryMailbox,
) -> Option<(u64, BTreeSet<String>)> {
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    let Some(generation) = settings::native::live_generation(&client) else {
        diagnostics.record(format!(
            "read skipped: state={:?} has no live generation",
            client.state()
        ));
        return None;
    };
    diagnostics.record(format!(
        "read sent: noded.props.get services.registered generation={generation}"
    ));
    let response = tokio::time::timeout(
        SEND_TIMEOUT,
        client.call(
            "noded",
            "noded.props.get",
            json!({"path":"services.registered"}),
        ),
    )
    .await;
    let value = match response {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            diagnostics.record(format!("read failed generation={generation}: {error}"));
            tracing::warn!(generation, %error, "scene host registry read failed");
            return None;
        }
        Err(_) => {
            diagnostics.record(format!(
                "read timed out generation={generation} budget={SEND_TIMEOUT:?}"
            ));
            tracing::warn!(generation, "scene host registry read timed out");
            return None;
        }
    };
    let Some(names) = value.as_array() else {
        diagnostics.record(format!(
            "read rejected: expected string array, received {value}"
        ));
        tracing::warn!(generation, "scene host registry read returned a non-array");
        return None;
    };
    diagnostics.record(format!(
        "read result generation={generation} array_size={}",
        names.len()
    ));
    let Some(services) = names
        .iter()
        .map(|name| name.as_str().map(str::to_owned))
        .collect::<Option<BTreeSet<_>>>()
    else {
        diagnostics.record("read rejected: array contains a non-string service name");
        tracing::warn!(
            generation,
            "scene host registry read returned a non-string service name"
        );
        return None;
    };
    let live = settings::native::live_generation(&client);
    if live != Some(generation) {
        diagnostics.record(format!(
            "read dropped: generation={generation} live={live:?}"
        ));
        tracing::warn!(
            generation,
            ?live,
            "scene host registry read generation mismatch"
        );
        return None;
    }
    diagnostics.record(format!(
        "read accepted generation={generation} services={services:?}"
    ));
    Some((generation, services))
}

async fn registry_subscribe(client: Arc<SupervisedClient>, diagnostics: RegistryMailbox) -> bool {
    let mut attempt = 0;
    loop {
        let Some(generation) = settings::native::live_generation(&client) else {
            diagnostics.record(format!(
                "subscribe skipped: state={:?} has no live generation",
                client.state()
            ));
            return false;
        };
        diagnostics.record(format!(
            "subscribe sent topic={REGISTRY_TOPIC} generation={generation} attempt={attempt}"
        ));
        match tokio::time::timeout(SEND_TIMEOUT, client.subscribe_topic(REGISTRY_TOPIC)).await {
            Ok(Ok(())) => {
                diagnostics.record(format!(
                    "subscribe ok topic={REGISTRY_TOPIC} generation={generation}"
                ));
                return true;
            }
            Ok(Err(error)) => {
                diagnostics.record(format!("subscribe failed generation={generation}: {error}"));
                tracing::warn!(
                    generation,
                    %error,
                    "scene host registry subscription failed; retrying"
                );
            }
            Err(_) => {
                diagnostics.record(format!(
                    "subscribe timed out generation={generation} budget={SEND_TIMEOUT:?}"
                ));
                tracing::warn!(
                    generation,
                    "scene host registry subscription timed out; retrying"
                );
            }
        }
        tokio::time::sleep(RETRY_INITIAL * (1 << attempt.min(3))).await;
        attempt = (attempt + 1).min(3);
    }
}

/// Answer a request the engine never sees.
async fn refuse(client: &SupervisedClient, command: &IncomingCommand, rc: u8, body: Value) {
    let text = body.to_string();
    let reply = client.respond(command, rc, &text);
    match tokio::time::timeout(SEND_TIMEOUT, reply).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::debug!(%error, "scene host refusal not delivered"),
        Err(_) => tracing::debug!("scene host refusal timed out"),
    }
}

fn admit(
    service: &str,
    delivery: &Delivery,
    command: IncomingCommand,
) -> Option<(IncomingCommand, u8, Value)> {
    match route(service, &command) {
        Route::Describe => return Some((command, 0, json!(SceneCommand::manifest(service)))),
        Route::Scene(kind) => {
            let request = Request {
                kind,
                from: command.from.clone(),
                command: command.command.clone(),
                id: command.id.clone(),
                body: command.body.clone(),
                headers: command.headers.clone(),
                generation: command.generation,
            };
            if !delivery.send(Inbound::Request(request)) {
                let body = json!({"error_code":"QUEUE_FULL","message":"scene host queue full"});
                return Some((command, 11, body));
            }
        }
        // `serve` handles invalidations with a generation-fenced registry
        // read. A diff must never become an engine snapshot directly.
        Route::Live => {}
        Route::Forged => {
            tracing::warn!(from = %command.from, "scene host: ignoring a registry claim not from the local broker")
        }
        Route::Unknown => {
            let message = format!("{} is not a scene host command", command.command);
            return Some((
                command,
                10,
                json!({"error_code":"UNKNOWN_COMMAND","message":message}),
            ));
        }
        Route::Ignore(_) => {}
    }
    None
}

/// One `settings.apply` to settingsd, bounded by [`EVENT_TIMEOUT`].
async fn settings_apply(client: &SupervisedClient, body: &Value) -> Result<Value, Value> {
    let (headers, body) = (BTreeMap::new(), body.to_string());
    let call = client.call_with_headers_raw("settingsd", "settings.apply", &headers, &body);
    match tokio::time::timeout(EVENT_TIMEOUT, call).await {
        Err(_) => Err(json!({"error_code":"SETTINGS_UNAVAILABLE",
            "message":"settingsd did not answer settings.apply in time"})),
        Ok(Err(error)) => Err(json!({"error_code":"SETTINGS_UNAVAILABLE",
            "message":format!("settings.apply could not reach settingsd: {error}")})),
        Ok(Ok((rc, reply, error))) => settings_outcome(rc, &reply, error.as_deref()),
    }
}

/// settingsd's `settings.apply` reply as an outcome: its body when accepted;
/// a refusal settingsd made (a `status`) as `SETTINGS_CONFLICT` (the fence
/// failed) or `SETTINGS_REFUSED`; anything else, such as the broker finding
/// no settingsd, as `SETTINGS_UNAVAILABLE`.
pub(crate) fn settings_outcome(rc: u8, reply: &str, error: Option<&str>) -> Result<Value, Value> {
    let parsed: Value = serde_json::from_str(reply).unwrap_or(Value::Null);
    if rc == 0 && parsed.is_object() {
        return Ok(parsed);
    }
    let status = parsed["status"].as_str();
    let (code, message) = match status {
        Some("conflict") => (
            "SETTINGS_CONFLICT",
            "the settings changed since the shell applied them; read the panel state and retry"
                .to_owned(),
        ),
        Some(status) if rc != 0 => (
            "SETTINGS_REFUSED",
            format!("settingsd refused the change: {status}"),
        ),
        _ => (
            "SETTINGS_UNAVAILABLE",
            format!(
                "settings.apply failed (rc {rc}): {}",
                error.filter(|e| !e.is_empty()).unwrap_or(reply)
            ),
        ),
    };
    let mut body = json!({"error_code":code, "message":message});
    if !parsed.is_null() {
        body["settings"] = parsed;
    }
    Err(body)
}

async fn send(
    client: &SupervisedClient,
    (scene_topic, panel_topic): &(String, String),
    message: Outbound,
) {
    let topic = match &message {
        Outbound::Publish { panel: true, .. } => panel_topic.as_str(),
        _ => scene_topic.as_str(),
    };
    let result = match &message {
        Outbound::Titlebars(_) | Outbound::WakeAfter(_) | Outbound::Settings { .. } => Ok(()),
        Outbound::Reply {
            generation,
            command,
            ..
        } if *generation != client.connection_generation() => {
            // Its caller's connection is gone; a reply on the new one would
            // reach whoever holds that name now, under a stale id.
            tracing::debug!(command = %command, "scene host reply dropped: admitted on an earlier connection");
            Ok(())
        }
        Outbound::Reply {
            generation,
            to,
            command,
            id,
            rc,
            body,
            ..
        } => tokio::time::timeout(
            SEND_TIMEOUT,
            client.respond_parts(*generation, to, command, id.as_deref(), *rc, body),
        )
        .await
        .map_err(|_| "timed out".to_string())
        .and_then(|result| result.map_err(|error| error.to_string())),
        Outbound::Publish { panel, wire } => {
            let mut headers = BTreeMap::new();
            headers.insert("name".to_string(), topic.to_string());
            headers.insert("retain".to_string(), panel.to_string());
            match tokio::time::timeout(
                SEND_TIMEOUT,
                client.call_with_headers_raw("noded", "topic.publish", &headers, wire),
            )
            .await
            {
                Err(_) => Err("timed out".to_string()),
                Ok(Err(error)) => Err(error.to_string()),
                Ok(Ok((0, _, _))) => Ok(()),
                Ok(Ok((rc, body, _))) => {
                    Err(format!("topic.publish rejected with rc {rc}: {body}"))
                }
            }
        }
    };
    if let Err(error) = result {
        match message {
            Outbound::Titlebars(_) | Outbound::WakeAfter(_) | Outbound::Settings { .. } => {}
            Outbound::Reply { to, command, .. } => {
                tracing::warn!(%error, to = %to, command = %command, "scene host reply not delivered")
            }
            Outbound::Publish { .. } => tracing::warn!(%error, topic, "scene host publish failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_outcomes_name_conflicts_refusals_and_an_absent_settingsd() {
        let accepted = r#"{"status":"changed","receipt":{"revision":"2"}}"#;
        assert_eq!(
            settings_outcome(0, accepted, None).unwrap()["receipt"]["revision"],
            "2"
        );
        for (rc, reply, error, code) in [
            (
                10,
                r#"{"status":"conflict","revision":"3"}"#,
                None,
                "SETTINGS_CONFLICT",
            ),
            (
                10,
                r#"{"status":"validation_failed","path":"shell.panels.bottom"}"#,
                None,
                "SETTINGS_REFUSED",
            ),
            (
                10,
                "",
                Some("no such service: settingsd"),
                "SETTINGS_UNAVAILABLE",
            ),
            (0, "not json", None, "SETTINGS_UNAVAILABLE"),
        ] {
            let refusal = settings_outcome(rc, reply, error).unwrap_err();
            assert_eq!(refusal["error_code"], code, "{reply}");
            assert!(refusal["message"].as_str().is_some_and(|m| !m.is_empty()));
        }
        let refusal =
            settings_outcome(10, r#"{"status":"conflict","revision":"3"}"#, None).unwrap_err();
        assert_eq!(refusal["settings"]["revision"], "3");
        let absent = settings_outcome(10, "", Some("no such service: settingsd")).unwrap_err();
        assert!(
            absent["message"]
                .as_str()
                .unwrap()
                .contains("no such service")
        );
    }

    #[test]
    fn titlebar_changes_require_the_registered_local_publisher() {
        let topic = "prefs.ui.titlebar.changed";
        let mut command = incoming(
            "",
            topic,
            None,
            &[
                ("topic", topic),
                ("broker_service", "prefs"),
                ("broker_origin", "local"),
            ],
            "{}",
        );
        let services = vec!["prefs".into()];
        assert_eq!(titlebar_change(&command, &services), Some("prefs"));
        assert_eq!(titlebar_change(&command, &[]), None);
        command
            .headers
            .insert("broker_service".into(), "other".into());
        assert_eq!(titlebar_change(&command, &services), None);
        command
            .headers
            .insert("broker_service".into(), "prefs".into());
        command
            .headers
            .insert("broker_origin".into(), "remote".into());
        assert_eq!(titlebar_change(&command, &services), None);
    }

    #[test]
    fn services_count_only_burst_keeps_inflight_read_and_converges_with_one_followup() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        async fn read(
            starts: Arc<AtomicUsize>,
            revision: Arc<AtomicUsize>,
            release: tokio::sync::oneshot::Receiver<()>,
        ) -> usize {
            starts.fetch_add(1, Ordering::SeqCst);
            let snapshot = revision.load(Ordering::SeqCst);
            release.await.unwrap();
            snapshot
        }

        let starts = Arc::new(AtomicUsize::new(0));
        let revision = Arc::new(AtomicUsize::new(0));
        let mut registry = RegistryRead::new();
        let mut context = Context::from_waker(Waker::noop());
        let (finish, release) = tokio::sync::oneshot::channel();
        registry.request(|| read(starts.clone(), revision.clone(), release));
        assert!(
            registry
                .pending
                .as_mut()
                .unwrap()
                .as_mut()
                .poll(&mut context)
                .is_pending()
        );
        for next in 1..=100 {
            revision.store(next, Ordering::SeqCst);
            let event = incoming(
                "",
                "props.changed",
                None,
                &[
                    ("topic", REGISTRY_TOPIC),
                    ("broker_service", "noded"),
                    ("broker_origin", "local"),
                    ("path", "services.count"),
                ],
                &format!(r#"{{"path":"services.count","new":{next}}}"#),
            );
            match route("shell", &event) {
                Route::Live => {
                    registry.request(|| panic!("churn must not replace an in-flight read"));
                }
                other => panic!("a count-only change must invalidate the registry: {other:?}"),
            }
        }
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(!finish.is_closed(), "the original read was cancelled");
        finish.send(()).unwrap();
        assert_eq!(
            registry
                .pending
                .as_mut()
                .unwrap()
                .as_mut()
                .poll(&mut context),
            Poll::Ready(0)
        );
        assert!(registry.complete(), "the burst owes one follow-up read");

        let (finish, release) = tokio::sync::oneshot::channel();
        registry.request(|| read(starts.clone(), revision.clone(), release));
        assert!(
            registry
                .pending
                .as_mut()
                .unwrap()
                .as_mut()
                .poll(&mut context)
                .is_pending()
        );
        finish.send(()).unwrap();
        assert_eq!(
            registry
                .pending
                .as_mut()
                .unwrap()
                .as_mut()
                .poll(&mut context),
            Poll::Ready(100)
        );
        assert!(
            !registry.complete(),
            "a settled burst must not keep reading"
        );
        assert!(registry.pending.is_none());
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn registry_baseline_waits_for_live_evidence_and_fences_old_connections() {
        let registry = RegistryMailbox::default();
        registry.store(5, BTreeSet::from(["prefs".into()]));
        assert!(take_registry(&registry, None).is_none());
        assert!(
            matches!(take_registry(&registry, Some(5)), Some(Inbound::Live(names)) if names.contains("prefs"))
        );
        assert!(take_registry(&registry, Some(5)).is_none());
        registry.store(5, BTreeSet::from(["old-app".into()]));
        assert!(take_registry(&registry, Some(6)).is_none());
        registry.store(6, BTreeSet::from(["prefs".into()]));
        assert!(
            matches!(take_registry(&registry, Some(6)), Some(Inbound::Live(names)) if names.contains("prefs"))
        );
    }

    fn menu_registry_port(
        url: &str,
        cache: &std::path::Path,
    ) -> (Port, tokio_mpsc::UnboundedReceiver<()>) {
        let (notify, wake) = tokio_mpsc::unbounded_channel();
        let port = Port::start_at(
            HostConfig {
                noded_url: url.into(),
                service_override: None,
            },
            Arc::new(move || {
                let _ = notify.send(());
            }),
            cache.to_path_buf(),
        )
        .unwrap();
        (port, wake)
    }

    async fn wait_menu_registry(
        port: &Port,
        wake: &mut tokio_mpsc::UnboundedReceiver<()>,
        has_app: bool,
        phase: &str,
    ) -> BTreeSet<String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        port.registry
            .record(format!("test wait phase={phase} expected_prefs={has_app}"));
        loop {
            while let Some(message) = port.try_recv() {
                port.registry.record(format!("test received {message:?}"));
                match message {
                    Inbound::Live(names) if names.contains("prefs") == has_app => {
                        eprintln!("registry phase={phase} converged: {names:?}");
                        return names;
                    }
                    Inbound::Refused(reason) => panic!(
                        "registry phase={phase}: scene host refused: {reason}\n{}",
                        port.registry.history()
                    ),
                    _ => {}
                }
            }
            // Yield this current-thread runtime so the test citizen's reader,
            // supervisor and close tasks can run while the host worker waits.
            match tokio::time::timeout_at(deadline, wake.recv()).await {
                Ok(Some(())) => port.registry.record("test received wake"),
                outcome => {
                    // Distinguish a missed invalidation from a citizen whose
                    // socket/name is still alive at the broker.
                    let broker_registry = if let Some(client) = port.client.get() {
                        registry_read(Arc::clone(client), Duration::ZERO, port.registry.clone())
                            .await
                    } else {
                        None
                    };
                    let lifecycle = port
                        .client
                        .get()
                        .map(|client| (client.state(), client.connection_generation()));
                    let mailbox = {
                        let state = port.registry.0.lock().unwrap();
                        state.pending.as_ref().map(|(generation, names)| {
                            (*generation, names.len(), names.contains("prefs"))
                        })
                    };
                    panic!(
                        "menu registry did not converge: phase={phase} expected_prefs={has_app} \
                         wake={outcome:?} lifecycle={lifecycle:?} live_generation={:?} \
                         pending_mailbox=(generation,count,prefs)={mailbox:?} \
                         broker_registry={broker_registry:?}\n\
                         last {REGISTRY_HISTORY} registry events:\n{}",
                        port.settings_generation(),
                        port.registry.history()
                    );
                }
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires private noded at MIXOS_NODED_URL with shell/prefs names free; use settings_test.mix"]
    async fn settings_menu_registry_covers_both_startup_orders_and_host_restart() {
        let url = std::env::var("MIXOS_NODED_URL").unwrap();
        let cache = tempfile::tempdir().unwrap();
        for app_first in [false, true] {
            let case = if app_first { "app-first" } else { "host-first" };
            let mut app = if app_first {
                Some(
                    SupervisedClient::connect_options("prefs", &url)
                        .connect()
                        .await
                        .unwrap(),
                )
            } else {
                None
            };
            let (mut port, mut wake) = menu_registry_port(&url, cache.path());
            // No scenes have been loaded: the baseline is the whole broker
            // registry, including an app already running before compd starts.
            let names =
                wait_menu_registry(&port, &mut wake, app_first, &format!("{case}/baseline")).await;
            assert!(names.contains("shell"));
            if !app_first {
                app = Some(
                    SupervisedClient::connect_options("prefs", &url)
                        .connect()
                        .await
                        .unwrap(),
                );
                // Production noded publishes without `from`, using
                // broker_service. The pre-fix route misses this arrival.
                wait_menu_registry(&port, &mut wake, true, "host-first/app-arrival").await;
            }
            port.finish();
            let (mut restarted, mut wake) = menu_registry_port(&url, cache.path());
            // The app did not reconnect. A subscribe-only host cannot pass:
            // the registry topic is not retained; it needs an initial read.
            wait_menu_registry(&restarted, &mut wake, true, &format!("{case}/host-restart")).await;
            app.unwrap().close().await;
            wait_menu_registry(
                &restarted,
                &mut wake,
                false,
                &format!("{case}/app-departure"),
            )
            .await;
            restarted.finish();
        }
    }

    #[test]
    fn help_is_a_descriptor_list_of_answered_commands() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let delivery = Delivery {
            sender,
            waker: Arc::new(|| {}),
        };
        for service in ["shell", "example-shell"] {
            let request = incoming("tester", "HELP", Some("1"), &[], "{}");
            let (_, rc, body) = admit(service, &delivery, request).unwrap();
            assert_eq!(rc, 0);
            let entries: Vec<bus::CommandDescriptor> =
                serde_json::from_value(body.clone()).unwrap();
            assert_eq!(entries[0].name, "HELP");
            let request = incoming("tester", "app.describe", Some("1"), &[], "{}");
            assert_eq!(admit(service, &delivery, request).unwrap().2, body);
            for entry in entries {
                let request = incoming("tester", &entry.name, Some("2"), &[], "{}");
                if let Some((_, rc, body)) = admit(service, &delivery, request) {
                    assert_eq!(rc, 0, "{}: {body}", entry.name);
                } else {
                    assert!(matches!(receiver.try_recv().unwrap(), Inbound::Request(_)));
                }
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires private broker; use settings_test.mix"]
    async fn settings_titlebar_property_and_registry_execution_use_the_scene_port() {
        async fn changed(client: &SupervisedClient, revision: u64) -> u8 {
            let topic = "prefs.ui.titlebar.changed";
            let headers = BTreeMap::from([("name".into(), topic.into())]);
            let wire = format!(
                "---\ncommand: {topic}\n---\n{}",
                json!({"revision": revision})
            );
            let (rc, _, _) = client
                .call_with_headers_raw("noded", "topic.publish", &headers, &wire)
                .await
                .unwrap();
            rc
        }

        async fn wait_bar(
            port: &Port,
            wake: &mut tokio_mpsc::UnboundedReceiver<()>,
            revision: u64,
            reads: &AtomicUsize,
            phase: &str,
        ) {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            let mut last = None;
            port.registry
                .record(format!("test titlebar phase={phase} revision={revision}"));
            loop {
                while let Some(message) = port.try_recv() {
                    port.registry.record(format!("test received {message:?}"));
                    match message {
                        Inbound::Titlebar { service, form, .. } => {
                            assert_eq!(service, "prefs");
                            if form["bar"]["revision"] == revision {
                                assert_eq!(form["bar"]["items"][0]["command"], "file.save");
                                return;
                            }
                            last = Some(form);
                        }
                        Inbound::Refused(reason) => panic!(
                            "titlebar phase={phase}: scene host refused: {reason}\n{}",
                            port.registry.history()
                        ),
                        _ => {}
                    }
                }
                match tokio::time::timeout_at(deadline, wake.recv()).await {
                    Ok(Some(())) => port.registry.record("test received titlebar wake"),
                    outcome => {
                        let broker_registry = if let Some(client) = port.client.get() {
                            registry_read(Arc::clone(client), Duration::ZERO, port.registry.clone())
                                .await
                        } else {
                            None
                        };
                        let lifecycle = port
                            .client
                            .get()
                            .map(|client| (client.state(), client.connection_generation()));
                        panic!(
                            "titlebar phase={phase} revision={revision}: {outcome:?}; reads={} lifecycle={lifecycle:?} targets={:?} last={last:?} broker={broker_registry:?}\n{}",
                            reads.load(Ordering::SeqCst),
                            port.titlebars,
                            port.registry.history()
                        );
                    }
                }
            }
        }

        let url = std::env::var("MIXOS_NODED_URL").unwrap();
        let app = Arc::new(
            SupervisedClient::connect_options("prefs", &url)
                .connect()
                .await
                .unwrap(),
        );
        let mut requests = app.incoming().unwrap();
        let (executed, mut runs) = tokio_mpsc::unbounded_channel();
        let revision = Arc::new(std::sync::atomic::AtomicU64::new(7));
        let reads = Arc::new(AtomicUsize::new(0));
        let peer = Arc::clone(&app);
        let peer_revision = Arc::clone(&revision);
        let peer_reads = Arc::clone(&reads);
        let responder = tokio::spawn(async move {
            while let Some(request) = requests.recv().await {
                let body = if request.command == "prefs.ui.titlebar.form" {
                    peer_reads.fetch_add(1, Ordering::SeqCst);
                    json!({"hosted":true,"service":"prefs","bar":{
                        "revision":peer_revision.load(Ordering::SeqCst),"items":[
                            {"command":"file.save","icon":"save","tooltip":"Save","enabled":true,"toggled":false}
                        ]
                    }})
                } else {
                    if request.command == "prefs.execute" {
                        let _ = executed.send(request.body.clone());
                    }
                    json!({"ok":true})
                };
                peer.respond(&request, 0, &body.to_string()).await.unwrap();
            }
        });
        let cache = tempfile::tempdir().unwrap();
        let (mut port, mut wake) = menu_registry_port(&url, cache.path());
        wait_menu_registry(&port, &mut wake, true, "action-app-first").await;
        port.titlebar_services(vec!["prefs".into()]);
        wait_bar(&port, &mut wake, 7, &reads, "registration").await;
        tokio::time::sleep(Duration::from_millis(750)).await;
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "unchanged bars are not polled",
        );

        let other = SupervisedClient::connect_options("other", &url)
            .connect()
            .await
            .unwrap();
        assert_ne!(
            changed(&other, 8).await,
            0,
            "the broker reserves the publisher"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "another publisher cannot invalidate prefs",
        );
        revision.store(8, Ordering::SeqCst);
        assert_eq!(changed(&app, 8).await, 0);
        wait_bar(&port, &mut wake, 8, &reads, "change").await;
        assert_eq!(reads.load(Ordering::SeqCst), 2);

        assert!(port.emit("prefs", "prefs.execute", json!({"id":"file.save"})));
        let body = tokio::time::timeout(Duration::from_secs(5), runs.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({"id":"file.save"}),
        );
        port.finish();
        responder.abort();
        other.close().await;
        app.close().await;
    }

    fn install_settings_fonts() {
        // egui contexts carry their own fonts; nothing is installed process-wide.
    }

    fn settings_drive(
        port: &Port,
        wake: &Receiver<()>,
        session: &mut SettingsUi<Arc<Look>>,
        panels: &mut crate::panels::Panels,
        revision: Option<u64>,
    ) -> usize {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut changes = 0;
        loop {
            changes += session
                .drain_with(
                    || port.settings_generation(),
                    |presentation| {
                        panels.set_preferences(presentation.content().preferences.clone());
                    },
                )
                .len();
            let ready =
                match revision {
                    Some(revision) => {
                        session.session().host().kind()
                            == Some(settings::fallback::PresentationKind::Current)
                            && session.session().host().consumer().applied().is_some_and(
                                |snapshot| snapshot.revision == settings::Revision(revision),
                            )
                    }
                    None => {
                        session.session().host().kind()
                            == Some(settings::fallback::PresentationKind::Embedded)
                    }
                };
            if ready {
                return changes;
            }
            wake.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("Quoin settings did not converge before deadline");
        }
    }

    fn settings_port(
        url: String,
        cache_directory: std::path::PathBuf,
    ) -> (Port, Receiver<()>, SettingsUi<Arc<Look>>) {
        install_settings_fonts();
        let (notify, wake) = mpsc::channel();
        let mut port = Port::start_at(
            HostConfig {
                service_override: None,
                noded_url: url,
            },
            Arc::new(move || {
                let _ = notify.send(());
            }),
            cache_directory,
        )
        .unwrap();
        let mut session = port.take_settings_ui();
        session.reconcile(port.settings_generation());
        (port, wake, session)
    }

    #[test]
    #[ignore = "requires settings_test.mix isolated environment"]
    fn settings_offline_port_prepares_fallback_while_connecting() {
        // Own an unresponsive endpoint that never completes the WS upgrade,
        // without selecting a port
        // somebody else can claim between binding and the connection attempt.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let cache = tempfile::tempdir().unwrap();
        let (port, wake, mut session) = settings_port(
            format!("ws://{}/ws", listener.local_addr().unwrap()),
            cache.path().join("settings"),
        );
        let mut panels = crate::panels::Panels::default();
        assert_eq!(
            settings_drive(&port, &wake, &mut session, &mut panels, None),
            1
        );
        panels.ensure("fixture", (1280.0, 800.0));
        assert_eq!(
            panels.state("fixture", crate::seat::Edge::Bottom)["mode"],
            "docked"
        );
        assert_eq!(port.settings_generation(), None);
        assert_eq!(
            session
                .session()
                .host()
                .presentation()
                .unwrap()
                .content()
                .chrome(decor::ChromeStyle::Mac)
                .tokens,
            decor::TokenSource::Prepared
        );
        let closing = std::time::Instant::now();
        port.finish();
        assert!(closing.elapsed() < SHUTDOWN_GRACE + Duration::from_millis(100));
    }

    #[tokio::test]
    #[ignore = "requires exact-revision binaries and isolated settings_test.mix broker"]
    async fn settings_native_port_activates_and_retains_last_good_after_real_loss() {
        use std::process::{Child, Command, Stdio};
        struct Authority(Child);
        impl Drop for Authority {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let root = tempfile::tempdir().unwrap();
        let binary = std::env::var("MIXOS_TEST_SETTINGSD").unwrap();
        assert!(
            Command::new(&binary)
                .args(["seed", "--allow-create", "--instance", "fixture", "--root"])
                .arg(root.path())
                .status()
                .unwrap()
                .success()
        );
        let log = std::fs::File::create(root.path().join("settingsd.log")).unwrap();
        let _authority = Authority(
            Command::new(&binary)
                .args(["serve", "--instance", "fixture", "--root"])
                .arg(root.path())
                .stdout(Stdio::from(log.try_clone().unwrap()))
                .stderr(Stdio::from(log))
                .spawn()
                .unwrap(),
        );
        let url = std::env::var("MIXOS_NODED_URL").unwrap();
        let cache_directory = root.path().join("cache/settings");
        let (port, wake, mut session) = settings_port(url.clone(), cache_directory.clone());
        let mut panels = crate::panels::Panels::default();
        panels.ensure("fixture", (1280.0, 800.0));
        assert_eq!(
            settings_drive(&port, &wake, &mut session, &mut panels, Some(1)),
            1
        );
        let before = session
            .session()
            .host()
            .presentation()
            .unwrap()
            .content()
            .clone();
        let controller = SupervisedClient::connect_options("quoin-settings-controller", &url)
            .connect()
            .await
            .unwrap();
        let current = session.session().host().consumer().current().unwrap();
        let applied = controller
            .call(
                "settingsd",
                "settings.apply",
                json!({
                    "binding": current.binding, "expected_incarnation": current.incarnation,
                    "expected_revision":"1", "operation_id":"quoin-live-fixture",
                    "changes":{"appearance.mode":"light","ui.text_scale":1.5,
                        "shell.panels.bottom":{"edge":"bottom","mode":"dock","thickness":80},
                        "shell.panels.top":{"edge":"right","mode":"overlay","thickness":160}}
                }),
            )
            .await
            .unwrap();
        assert_eq!(applied["status"], "changed");
        assert_eq!(
            settings_drive(&port, &wake, &mut session, &mut panels, Some(2)),
            1
        );
        assert_eq!(
            panels.state("fixture", crate::seat::Edge::Bottom)["mode"],
            "docked"
        );
        assert_eq!(
            panels.state("fixture", crate::seat::Edge::Bottom)["settings"]["requested_px"],
            80.0
        );
        assert_eq!(
            panels.state("fixture", crate::seat::Edge::Right)["mode"],
            "pinned"
        );
        panels.ensure("future", (1920.0, 1080.0));
        // A format-4 thickness reads at its nearest v2 step (160 → 80).
        assert_eq!(
            panels.state("future", crate::seat::Edge::Right)["settings"]["requested_px"],
            80.0
        );
        assert!(panels.zones("future").is_empty());
        let after = session
            .session()
            .host()
            .presentation()
            .unwrap()
            .content()
            .clone();
        assert_ne!(after.palette(true), before.palette(true));
        for style in decor::ChromeStyle::ALL {
            assert_eq!(
                after.chrome(style).deco.metrics.title_size_px,
                before.chrome(style).deco.metrics.title_size_px * 1.5
            );
        }
        let current = session.session().host().consumer().current().unwrap();
        let app_only = controller
            .call(
                "settingsd",
                "settings.apply",
                json!({
                    "binding":current.binding, "expected_incarnation":current.incarnation,
                    "expected_revision":"2", "operation_id":"quoin-unrelated-app",
                    "changes":{"apps.ced":{"mode":"light"}}
                }),
            )
            .await
            .unwrap();
        assert_eq!(app_only["status"], "changed");
        assert_eq!(
            settings_drive(&port, &wake, &mut session, &mut panels, Some(3)),
            0,
            "an unrelated app override acknowledges without staging shell resources"
        );
        let current = session.session().host().consumer().current().unwrap();
        let invalid = controller.call("settingsd", "settings.apply", json!({
            "binding":current.binding, "expected_incarnation":current.incarnation,
            "expected_revision":"3", "operation_id":"quoin-duplicate-edge",
            "changes":{"shell.panels.top":{"edge":"bottom","mode":"dock","thickness":100}}
        })).await.unwrap();
        assert_eq!(invalid["status"], "changed");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while session.session().host().consumer().fault().is_none() {
            let changes = session.drain_with(
                || port.settings_generation(),
                |presentation| {
                    panels.set_preferences(presentation.content().preferences.clone());
                },
            );
            assert!(
                changes.is_empty(),
                "failed whole preparation must not activate either resource"
            );
            if session.session().host().consumer().fault().is_none() {
                wake.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .expect("Quoin invalid-policy diagnostic did not arrive before deadline");
            }
        }
        assert_eq!(
            session
                .session()
                .host()
                .consumer()
                .applied()
                .unwrap()
                .revision,
            settings::Revision(3)
        );
        assert_eq!(
            panels.state("fixture", crate::seat::Edge::Bottom)["mode"],
            "docked"
        );
        assert_eq!(
            session
                .session()
                .host()
                .presentation()
                .unwrap()
                .content()
                .palette(true),
            after.palette(true)
        );
        let client = port.client.get().unwrap();
        client.close().await;
        assert_eq!(port.settings_generation(), None);
        let change = session.handle_with(SettingsEvent::Wake, port.settings_generation(), |_| {});
        assert!(change.is_none());
        assert_eq!(
            session.session().host().kind(),
            Some(settings::fallback::PresentationKind::LastGood)
        );
        assert_eq!(
            session
                .session()
                .host()
                .consumer()
                .applied()
                .unwrap()
                .revision,
            settings::Revision(3)
        );
        controller.close().await;
        port.finish();

        // The newest successfully activated snapshot survives connection loss
        // and shutdown, and is validated again by an offline worker.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let offline_url = format!("ws://{}/ws", listener.local_addr().unwrap());
        let (port, wake, mut cached) = settings_port(offline_url.clone(), cache_directory.clone());
        let mut cached_panels = crate::panels::Panels::default();
        drive_fallback(
            &port,
            &wake,
            &mut cached,
            &mut cached_panels,
            settings::fallback::PresentationKind::Cached,
        );
        assert_eq!(
            cached
                .session()
                .host()
                .consumer()
                .applied()
                .unwrap()
                .revision,
            settings::Revision(3)
        );
        assert_eq!(
            cached
                .session()
                .host()
                .presentation()
                .unwrap()
                .content()
                .palette(true),
            after.palette(true)
        );
        assert_eq!(port.settings_generation(), None);
        assert!(!cached.session().host().consumer().evidence().confirmed);
        port.finish();

        let files: Vec<_> = std::fs::read_dir(&cache_directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        assert_eq!(files.len(), 1);
        std::fs::write(&files[0], b"{broken cache").unwrap();
        let (port, wake, mut embedded) = settings_port(offline_url, cache_directory);
        drive_fallback(
            &port,
            &wake,
            &mut embedded,
            &mut cached_panels,
            settings::fallback::PresentationKind::Embedded,
        );
        assert!(!embedded.session().fallback_diagnostics().is_empty());
        assert!(!embedded.session().host().consumer().evidence().confirmed);
        port.finish();
    }

    fn drive_fallback(
        port: &Port,
        wake: &Receiver<()>,
        session: &mut SettingsUi<Arc<Look>>,
        panels: &mut crate::panels::Panels,
        kind: settings::fallback::PresentationKind,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            session.drain_with(
                || port.settings_generation(),
                |presentation| {
                    panels.set_preferences(presentation.content().preferences.clone());
                },
            );
            if session.session().host().kind() == Some(kind) {
                return;
            }
            wake.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("offline Quoin fallback did not converge");
        }
    }

    #[tokio::test]
    #[ignore = "requires isolated settings_test.mix broker"]
    async fn settings_initial_refusal_uses_only_configured_override_and_keeps_fallback() {
        install_settings_fonts();
        let url = std::env::var("MIXOS_NODED_URL").unwrap();
        let owner = SupervisedClient::connect_options("shell", &url)
            .connect()
            .await
            .unwrap();
        let owner_generation = owner.connection_generation();
        let root = tempfile::tempdir().unwrap();
        for (index, service_override) in [Some("shell-cache-fixture".to_owned()), None]
            .into_iter()
            .enumerate()
        {
            let (notify, wake) = mpsc::channel();
            let mut port = Port::start_at(
                HostConfig {
                    noded_url: url.clone(),
                    service_override,
                },
                Arc::new(move || {
                    let _ = notify.send(());
                }),
                root.path().join(index.to_string()),
            )
            .unwrap();
            let mut session = port.take_settings_ui();
            session.reconcile(port.settings_generation());
            let mut panels = crate::panels::Panels::default();
            drive_fallback(
                &port,
                &wake,
                &mut session,
                &mut panels,
                settings::fallback::PresentationKind::Embedded,
            );
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            loop {
                match port.try_recv() {
                    Some(Inbound::Registered(name)) => {
                        assert_eq!(index, 0);
                        assert_eq!(name, "shell-cache-fixture");
                        assert!(
                            port.connection_generation()
                                .is_some_and(|generation| generation > 0)
                        );
                        break;
                    }
                    Some(Inbound::Refused(reason)) => {
                        assert_eq!(index, 1);
                        assert!(reason.contains("no --scene-service"));
                        assert_eq!(port.connection_generation(), None);
                        break;
                    }
                    _ => {
                        wake.recv_timeout(
                            deadline.saturating_duration_since(std::time::Instant::now()),
                        )
                        .expect("initial scene-host refusal did not settle");
                    }
                }
            }
            assert_eq!(
                session.session().host().kind(),
                Some(settings::fallback::PresentationKind::Embedded)
            );
            port.finish();
            assert_eq!(
                owner.state(),
                ConnState::Connected,
                "the existing shell owner must remain untouched"
            );
            owner.call("noded", "noded.ping", json!({})).await.unwrap();
            assert_eq!(
                owner.connection_generation(),
                owner_generation,
                "fallback must preserve the existing owner's actual connection"
            );
        }
        owner.close().await;
    }

    #[test]
    fn page_request_from_worker_wakes_idle_loop_and_schedules_frame() {
        use dispatcher::state::{Dispatch, RedrawReason};
        use smithay::reexports::calloop::EventLoop;
        use smithay::reexports::calloop::ping::make_ping;
        use smithay::reexports::wayland_server::Display;

        struct Data {
            dispatch: Dispatch,
            host: crate::host::Host,
            frames: u32,
        }
        let mut event_loop: EventLoop<'static, Data> = EventLoop::try_new().unwrap();
        let display: Display<Dispatch> = Display::new().unwrap();
        let mut dispatch =
            dispatcher::wire::new_dispatch(&display.handle(), None, event_loop.handle());
        let (redraw, source) = make_ping().unwrap();
        dispatch.redraw.set_ping(redraw);
        dispatch.redraw.rendering("kms");
        dispatch.redraw.frame("kms", false);
        event_loop
            .handle()
            .insert_source(source, |_, _, data: &mut Data| {
                assert!(data.dispatch.redraw.pending());
                data.dispatch.redraw.rendering("kms");
                assert!(
                    data.dispatch
                        .redraw
                        .frame("kms", false)
                        .contains(RedrawReason::Publish)
                );
                data.frames += 1;
            })
            .unwrap();

        let mut host = crate::host::Host::default();
        host.panels.ensure("DP-1", (1280.0, 800.0));
        let request = |kind, body: Value| Request {
            kind,
            from: "scenes".into(),
            command: "shell.panel.page.set".into(),
            id: Some("1".into()),
            body: body.to_string(),
            headers: BTreeMap::from([("broker_origin".into(), "local".into())]),
            generation: 1,
        };
        let mut no_layout =
            |_: &crate::store::SceneStore, _: &str, _: Option<&str>| Err(Value::Null);
        for name in ["calendar", "notifications"] {
            let source = format!(
                "---\nscene: 1\nname: {name}\ncitizen: scenes\nwindow: {{\"kind\":\"edge\",\"edge\":\"right\"}}\n---\n```mix\nroot: {{widget: \"column\", children: []}}\n```\n"
            );
            let answer = host.answer(
                &request(SceneCommand::Load, json!({"source":source})),
                "DP-1",
                Some(1),
                &mut no_layout,
            );
            assert_eq!(answer.rc, 0);
        }
        host.panels.sync(&host.store);
        host.panels
            .page_set("DP-1", crate::seat::Edge::Right, "scene-calendar")
            .unwrap();

        // This is the production worker Delivery, including its after-enqueue
        // Ping. There are no input, frame clock, or watchdog sources to help it.
        let (sender, inbound) = mpsc::sync_channel(INBOUND_CAPACITY);
        let (ping, source) = make_ping().unwrap();
        let delivery = Arc::new(Delivery {
            sender,
            waker: Arc::new(move || ping.ping()),
        });
        event_loop
            .handle()
            .insert_source(source, move |_, _, data: &mut Data| {
                let Inbound::Request(request) = inbound.try_recv().unwrap() else {
                    panic!("expected page request")
                };
                let answer = data.host.answer(&request, "DP-1", Some(1), &mut no_layout);
                assert_eq!(answer.rc, 0);
                // lib::service uses exactly this changed verdict to request pixels.
                assert!(answer.changed);
                data.dispatch.schedule_redraw(RedrawReason::Publish);
            })
            .unwrap();
        let mut data = Data {
            dispatch,
            host,
            frames: 0,
        };
        for (index, page) in ["scene-notifications", "scene-calendar"]
            .into_iter()
            .enumerate()
        {
            let delivery = Arc::clone(&delivery);
            let request = request(
                SceneCommand::PanelPageSet,
                json!({"edge":"right", "id":page}),
            );
            let worker =
                std::thread::spawn(move || assert!(delivery.send(Inbound::Request(request))));
            event_loop
                .dispatch(Duration::from_secs(1), &mut data)
                .unwrap();
            assert_eq!(
                data.host.panels.state("DP-1", crate::seat::Edge::Right)["page"],
                page
            );
            assert!(data.dispatch.redraw.needs("kms"));
            event_loop
                .dispatch(Duration::from_secs(1), &mut data)
                .unwrap();
            worker.join().unwrap();
            assert_eq!(data.frames, index as u32 + 1);
        }
    }

    fn incoming(
        from: &str,
        command: &str,
        id: Option<&str>,
        headers: &[(&str, &str)],
        body: &str,
    ) -> IncomingCommand {
        IncomingCommand {
            generation: 0,
            from: from.into(),
            command: command.into(),
            id: id.map(str::to_owned),
            args: Value::Null,
            body: body.into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn shell_first_then_only_a_distinct_override() {
        assert_eq!(candidate_names(None), ["shell"]);
        assert_eq!(candidate_names(Some("shell")), ["shell"]);
        assert_eq!(candidate_names(Some("  ")), ["shell"]);
        assert_eq!(
            candidate_names(Some("shell-nested")),
            ["shell", "shell-nested"]
        );
    }

    #[test]
    fn frames_route_to_commands_registry_invalidations_or_nothing() {
        let load = incoming(
            "loader",
            "shell.scene.load",
            Some("1"),
            &[("broker_origin", "local")],
            "{}",
        );
        assert_eq!(route("shell", &load), Route::Scene(SceneCommand::Load));
        let unknown = incoming("loader", "shell.no.such.kind", Some("2"), &[], "{}");
        assert_eq!(route("shell", &unknown), Route::Unknown);
        let fire_and_forget = incoming("loader", "shell.no.such.kind", None, &[], "{}");
        assert_eq!(
            route("shell", &fire_and_forget),
            Route::Ignore("unknown_command_without_id")
        );
        let registry = r#"{"path":"services.registered","new":["comp","scenes"]}"#;
        let live = incoming(
            "noded",
            "",
            None,
            &[("topic", REGISTRY_TOPIC), ("broker_origin", "local")],
            registry,
        );
        assert_eq!(route("shell", &live), Route::Live);
        let mut stamped = incoming(
            "",
            "props.changed",
            None,
            &[("topic", REGISTRY_TOPIC), ("broker_service", "noded")],
            registry,
        );
        assert_eq!(route("shell", &stamped), route("shell", &live));
        stamped
            .headers
            .insert("broker_origin".into(), "mesh".into());
        assert_eq!(route("shell", &stamped), Route::Forged);
        stamped.headers.remove("broker_origin");
        stamped
            .headers
            .insert("broker_service".into(), "other".into());
        assert_eq!(
            route("shell", &stamped),
            Route::Ignore("missing_broker_identity")
        );
        let forged = incoming(
            "noded",
            "",
            None,
            &[("topic", REGISTRY_TOPIC), ("broker_origin", "mesh")],
            registry,
        );
        assert_eq!(route("shell", &forged), Route::Forged);
        let other_path = incoming(
            "noded",
            "",
            None,
            &[("topic", REGISTRY_TOPIC)],
            r#"{"path":"x","new":[]}"#,
        );
        assert_eq!(
            route("shell", &other_path),
            Route::Ignore("unrelated_property_path")
        );
        let mut count = incoming(
            "",
            "props.changed",
            None,
            &[
                ("topic", REGISTRY_TOPIC),
                ("broker_service", "noded"),
                ("broker_origin", "local"),
                ("path", "services.count"),
            ],
            r#"{"path":"services.count","old":2,"new":1}"#,
        );
        assert_eq!(route("shell", &count), Route::Live);
        count.body.clear();
        assert_eq!(
            route("shell", &count),
            Route::Live,
            "the trusted path header suffices"
        );
        count.headers.insert("broker_origin".into(), "mesh".into());
        assert_eq!(
            route("shell", &count),
            Route::Forged,
            "check origin before the path"
        );
        count.headers.insert("broker_origin".into(), "local".into());
        count.headers.remove("path");
        assert_eq!(
            route("shell", &count),
            Route::Ignore("invalid_payload_json")
        );
        count.body = "{}".into();
        assert_eq!(
            route("shell", &count),
            Route::Ignore("missing_or_invalid_path")
        );
        count.body = r#"{"path":"services.count","new":1}"#.into();
        assert_eq!(
            route("shell", &count),
            Route::Live,
            "body path fallback accepts counts"
        );
        let delivery = incoming(
            "x",
            "shell.scene.load",
            None,
            &[("topic", "x.changed")],
            "{}",
        );
        assert_eq!(
            route("shell", &delivery),
            Route::Ignore("unrelated_topic"),
            "a topic delivery is never a request"
        );
    }
}
