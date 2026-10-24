// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The running window: feeds the engine its events and carries out its
//! effects. Bus work runs on a small tokio runtime; each completion is sent
//! back as an [`Event`] with a repaint request, so the window wakes only
//! when something happened (no polling).
//!
//! **Ordering is the contract.** Each event is applied and its effects are
//! [`settle`]d before the next event is read, so a later delivery can never
//! change what an earlier command acts on. Within a frame, edits are applied
//! before commands ([`apply_ui`]), so a call uses the body the person sees.
//! Bus commands also wait behind window input accepted before them (the
//! toolkit drive layer), so `ui.type` then `workshop.call` calls with the
//! typed body; and every accepted command is answered before the Bus stops.
use crate::view::{UiEvent, view};
use crate::{label, strings};
use futures::StreamExt;
use inspector::bus::{self, CallError, Delivery, Handle, Reply};
use inspector::{Effect, Engine, Snapshot, model};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::task::Poll;
use toolkit::command::{CommandError, Origin};
use toolkit::{Registry, Strings, Theme, drive};

/// What finished off the UI thread.
pub enum Event {
    Measured(u64, Result<Reply, CallError>, String, u64),
    Stopped(Result<(), String>),
    MacroChanged,
    Workshop(u64, String, Value, Option<u64>, Result<Value, String>),
    Delivery(Delivery),
    Discovered(u64, Snapshot),
    Completed(u64, Result<Reply, CallError>),
    Shown(u64, Option<u64>, Result<Value, String>),
    /// A read of the session look (settingsd), for the shell.
    Look(u64, Result<Reply, CallError>),
    /// A retry of the session look after a transient fault (its generation).
    LookRetry(u64),
}

/// Apply one finished event to the engine.
pub fn apply_event(engine: &mut Engine, event: Event) {
    match event {
        Event::Delivery(delivery) => engine.delivery(delivery),
        Event::Discovered(ticket, snapshot) => engine.discovered(ticket, snapshot),
        Event::Completed(ticket, result) => engine.completed(ticket, result),
        Event::Measured(ticket, result, at, ms) => engine.completed_at(ticket, result, at, ms),
        Event::Shown(ticket, reply, result) => engine.shown(ticket, reply, result),
        // The session look is the shell's (see `Shell::looked`).
        Event::Look(..)
        | Event::LookRetry(_)
        | Event::Workshop(..)
        | Event::MacroChanged
        | Event::Stopped(_) => {}
    }
}

/// Apply one frame's interactions: edits first, then commands, so a command
/// sees every edit made in the same frame.
pub fn apply_ui(engine: &mut Engine, commands: &Registry<Engine>, events: Vec<UiEvent>) {
    // Dismissal wins over any toggle delivered in this frame.
    let (closed, events): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|e| matches!(e, UiEvent::CloseMacros));
    let (fired, edits): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|e| matches!(e, UiEvent::Command(..)));
    for event in edits.into_iter().chain(fired).chain(closed) {
        match event {
            UiEvent::MacroDestination(destination) => engine.workshop.destination = destination,
            UiEvent::MacroTarget(target) => engine.workshop.target = target,
            UiEvent::CancelDelete => engine.workshop.confirm_delete = false,
            UiEvent::Macro(operation, args) => {
                engine.workshop.arguments = args;
                let command = operation.replacen("macros.", "macro.", 1);
                let _ = commands.execute(&command, engine, Origin::Ui);
            }
            UiEvent::MacroDraft(text) => engine.workshop.draft = Some(text),
            UiEvent::Dock(open) => engine.workshop.dock_open = open,
            UiEvent::Pause(paused) => engine.workshop.paused = paused,
            UiEvent::TrafficFilter(filter) => engine.workshop.filter = filter,
            UiEvent::CloseMacros => engine.ui.macros_open = false,
            UiEvent::Command(id, origin) => {
                let _ = commands.execute(id, engine, origin);
            }
            UiEvent::Filter(filter) => engine.set_filter(filter),
            UiEvent::Toggle(key) => engine.toggle(&key),
            UiEvent::Select(row) => engine.select_row(&row),
            UiEvent::Body(body) => engine.set_body(body),
            UiEvent::Split(split) => engine.set_split(split),
            UiEvent::CloseDialog => engine.close_dialog(),
            UiEvent::FilterFocused => engine.filter_focused(),
            UiEvent::SetMode(mode) => engine.set_mode(mode),
        }
    }
}

/// The app prefix of Workshop's verbs, and of its drive verbs.
pub const APP: &str = "workshop";

/// The whole Bus surface: the engine's verbs and the window's drive verbs.
pub fn describe() -> Value {
    let mut surface = model::describe();
    if let Some(verbs) = surface["verbs"].as_array_mut() {
        verbs.extend([
            json!({"name":"workshop.traffic","args":{"limit":"optional integer 0..1024","filter":"optional string"},"description":"Read the bounded traffic ring","read_only":true}),
            json!({"name":"workshop.observe","args":{"on":"bool","filter":"optional observe filter","body":"none or redacted (default)"},"description":"Control broker observation; topic rows remain after refusal","read_only":false}),
            json!({"name":"workshop.macros.open","args":{"app":"app or global","id":"string"},"description":"Select a macro and read its document","read_only":false}),
        ]);
        verbs.extend(drive::describe(APP));
        verbs.extend(inspector::bus::macros::describe(APP));
    }
    surface
}

/// Resolve the effects that only need the registry (`workshop.commands`
/// and `workshop.execute`) or the description right now, in order, and
/// return the rest for the shell to perform. Running an executed command
/// here, before any later event, is what keeps its target the one selected
/// when it arrived.
pub fn settle(engine: &mut Engine, commands: &Registry<Engine>, strings: &Strings) -> Vec<Effect> {
    let mut out = Vec::new();
    loop {
        let effects = engine.take_effects();
        if effects.is_empty() {
            return out;
        }
        for effect in effects {
            match effect {
                Effect::Commands { id } => {
                    let body = json!({ "commands": commands.describe(engine, strings) });
                    out.push(Effect::Reply { id, rc: 0, body });
                }
                Effect::Execute { id, command } => {
                    let (rc, body) = match commands.execute(&command, engine, Origin::Bus) {
                        Ok(()) => (0, json!({ "executed": command })),
                        Err(CommandError::Denied(_)) => (
                            10,
                            json!({"error_code":"DENIED","message":label("command-denied")}),
                        ),
                        Err(CommandError::Unknown(_)) => (
                            10,
                            json!({"error_code":"UNKNOWN_COMMAND","message":label("unknown-command")}),
                        ),
                        Err(CommandError::Disabled(_)) => (
                            10,
                            json!({"error_code":"DISABLED","message":label("command-disabled")}),
                        ),
                    };
                    out.push(Effect::Reply { id, rc, body });
                }
                Effect::Describe { id, help } => {
                    let surface = describe();
                    let body = if help {
                        surface["verbs"].clone()
                    } else {
                        surface
                    };
                    out.push(Effect::Reply { id, rc: 0, body });
                }
                other => out.push(other),
            }
        }
    }
}

pub struct Shell {
    watched_topics: std::collections::BTreeSet<String>,
    macro_stopping: bool,
    macro_stopped: bool,
    _macro_change: macros::session::ChangeHook,
    engine: Engine,
    commands: Registry<Engine>,
    strings: Strings,
    theme: Theme,
    bus: Handle,
    comp: String,
    runtime: tokio::runtime::Runtime,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    ctx: egui::Context,
    exiting: bool,
    /// Bus commands waiting behind drive input accepted before them.
    held: VecDeque<Event>,
    /// The Bus deliveries not yet forwarded. Shutdown closes and drains it
    /// under its lock, which the forwarder holds while it hands one on, so
    /// every command is in this inbox or in `rx`, never between the two.
    inbox: Arc<Mutex<Deliveries>>,
    /// The theme choice installed last.
    installed: toolkit::theme_menu::Choice,
    /// The theme file's session theme: the fallback while settingsd is
    /// absent, or holds a look this build cannot draw.
    file_theme: Theme,
    /// The session look settingsd holds, once read.
    look: Option<settings::Appearance>,
    /// The profile's bindings for Workshop, as last applied.
    keymap: toolkit::keymap::Overrides,
    follower: settings::follow::Follower,
    /// The latest look retry; an older one firing is void.
    look_retry: u64,
}

/// This host's name: the settingsd instance to try first (settingsd runs
/// as `--instance %H`; the follower adopts the right one from its answer).
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}

/// How long one settings read may take.
const LOOK_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

fn timestamp() -> String {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = time.as_secs() % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        time.subsec_millis()
    )
}

/// What the engine shows for an unchosen axis: the session theme's.
fn session_of(theme: &Theme) -> inspector::Session {
    inspector::Session {
        scheme: theme.scheme(),
        style: theme.style_axis(),
        mode: theme.mode(),
        decorations: theme.requested_decorations(),
        captions: theme.captions(),
    }
}

type Deliveries = futures::channel::mpsc::Receiver<Delivery>;

fn locked(inbox: &Mutex<Deliveries>) -> std::sync::MutexGuard<'_, Deliveries> {
    inbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The egui id of the services panel in [`view`]: an agent-set split
/// clears its remembered width so the panel takes the new default.
const SERVICES_PANEL: &str = "services";

impl Shell {
    /// A window on `bus`, with its deliveries forwarded from `deliveries`.
    pub fn new(
        ctx: egui::Context,
        theme: Theme,
        bus: Handle,
        deliveries: futures::channel::mpsc::Receiver<Delivery>,
        comp: String,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("workshop-effects")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        drive::install(&ctx, APP);
        let (tx, rx) = mpsc::channel();
        let changed_tx = tx.clone();
        let changed_ctx = ctx.clone();
        let macro_change: macros::session::ChangeHook = Arc::new(move || {
            let _ = changed_tx.send(Event::MacroChanged);
            changed_ctx.request_repaint();
        });
        macros::Session::for_app("workshop").observe_changes(&macro_change);
        let forward = tx.clone();
        let wake = ctx.clone();
        let inbox = Arc::new(Mutex::new(deliveries));
        let source = inbox.clone();
        runtime.spawn(async move {
            loop {
                // Take one delivery and hand it on under the inbox lock.
                let next =
                    futures::future::poll_fn(|cx| match locked(&source).poll_next_unpin(cx) {
                        Poll::Ready(Some(delivery)) => {
                            Poll::Ready(Some(forward.send(Event::Delivery(delivery)).is_ok()))
                        }
                        Poll::Ready(None) => Poll::Ready(None),
                        Poll::Pending => Poll::Pending,
                    })
                    .await;
                match next {
                    Some(true) => wake.request_repaint(),
                    Some(false) => return,
                    None => break,
                }
            }
            let _ = forward.send(Event::Delivery(Delivery::Disconnected));
            wake.request_repaint();
        });
        let mut shell = Self {
            watched_topics: Default::default(),
            macro_stopping: false,
            macro_stopped: false,
            _macro_change: macro_change,
            engine: Engine::new(label),
            commands: crate::commands::registry(),
            strings: strings(),
            file_theme: theme.clone(),
            theme,
            bus,
            comp,
            runtime,
            tx,
            rx,
            ctx,
            exiting: false,
            held: VecDeque::new(),
            inbox,
            installed: toolkit::theme_menu::Choice::default(),
            look: None,
            keymap: toolkit::keymap::Overrides::new(),
            follower: settings::follow::Follower::new(&hostname()),
            look_retry: 0,
        };
        shell.engine.session = session_of(&shell.theme);
        shell.engine.queue_workshop("catalogues", json!({}), None);
        shell
            .engine
            .queue_workshop("observe", json!({"on":true}), None);
        shell.settle();
        shell.read_look();
        Ok(shell)
    }

    /// Spawn `work` and send its result back as an event.
    fn spawn<F>(&self, work: F)
    where
        F: Future<Output = Event> + Send + 'static,
    {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.runtime.spawn(async move {
            let _ = tx.send(work.await);
            ctx.request_repaint();
        });
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn exiting(&self) -> bool {
        self.exiting
    }

    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// Hand on every delivery still in the inbox now, in order, as the
    /// forwarder would: under the inbox lock it holds while it hands one on,
    /// so these go in behind whatever it has already forwarded. For tests
    /// whose next frame must see every delivery already sent.
    #[doc(hidden)]
    pub fn forward_pending(&self) {
        let mut inbox = locked(&self.inbox);
        while let Ok(delivery) = inbox.try_recv() {
            let _ = self.tx.send(Event::Delivery(delivery));
        }
    }

    /// Apply every finished event, settling after each one. A Bus command
    /// is held, in order, while drive input accepted before it is still
    /// unanswered; completions are applied at once.
    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            if let Event::Stopped(result) = event {
                match result {
                    Ok(()) => {
                        self.macro_stopped = true;
                        self.perform(Effect::Exit);
                    }
                    Err(error) => self.engine.workshop.message = error,
                }
                continue;
            }
            if let Event::MacroChanged = event {
                let status = macros::Session::for_app("workshop").status();
                if status["state"] == "running" && self.engine.workshop.subscription.is_some() {
                    self.engine.workshop.filter = format!(
                        "macro:workshop:{}",
                        status["id"].as_str().unwrap_or_default()
                    );
                }
                if !self.engine.quitting {
                    self.engine.queue_workshop("catalogues", json!({}), None);
                    self.settle();
                }
                continue;
            }
            if let Event::Workshop(ticket, operation, args, reply, result) = event {
                if !self.engine.workshop_completion(ticket, &operation) {
                    if let Some(id) = reply {
                        self.bus.reply(id, 10, json!({"error_code":"BUSY","message":"superseded by a newer selection"}));
                    }
                    self.engine.workshop_settled();
                    self.settle();
                    continue;
                }
                let body = result.unwrap_or_else(
                    |message| json!({"error_code":"UNAVAILABLE","message":message}),
                );
                let refused = body.get("error_code").is_some();
                if operation == "catalogues" && !refused {
                    self.engine.workshop.catalogues =
                        serde_json::from_value(body.clone()).unwrap_or_default();
                } else if operation == "macros.open" && !refused {
                    let app = args["app"].as_str().unwrap_or("global").to_owned();
                    self.engine.ui.selected = None;
                    if body.get("branch").is_some() {
                        self.engine.ui.expanded.insert(format!("macros:{app}"));
                        self.engine.ui.row_key = Some(format!("macros:{app}"));
                        self.engine.workshop.selected = None;
                        if let Some(id) = reply {
                            self.bus.reply(id, 0, body);
                        }
                        self.engine.workshop_settled();
                        self.settle();
                        continue;
                    }
                    let id = args["id"].as_str().unwrap_or_default().to_owned();
                    if let Some(catalogue) = body.get("catalogue") {
                        self.engine
                            .workshop
                            .catalogues
                            .insert(app.clone(), catalogue.clone());
                    }
                    self.engine.ui.row_key = Some(format!("macro:{app}:{id}"));
                    self.engine.ui.expanded.insert(format!("macros:{app}"));
                    self.engine.workshop.selected = Some((app, id));
                    self.engine.reveal_macro();
                    self.engine.workshop.document = body.clone();
                    self.engine.workshop.draft = None;
                    self.engine.workshop.saved_text = None;
                } else if operation == "observe" {
                    self.engine.workshop.observe = if refused {
                        body["message"].as_str().unwrap_or_default().into()
                    } else if args["on"] == false {
                        label("traffic-observe-off")
                    } else {
                        body["body"].as_str().unwrap_or("redacted").into()
                    };
                    if !refused {
                        self.engine.workshop.subscription =
                            body["subscription_id"].as_str().map(str::to_owned);
                    }
                } else {
                    self.engine.workshop.message = body.to_string();
                    if operation == "macros.put"
                        && !refused
                        && self
                            .engine
                            .workshop
                            .selected
                            .as_ref()
                            .is_some_and(|(app, id)| {
                                app == args["app"].as_str().unwrap_or("global")
                                    && Some(id.as_str()) == args["id"].as_str()
                            })
                    {
                        self.engine.workshop.saved_text = args["text"].as_str().map(str::to_owned);
                        self.engine.workshop.document["text"] = args["text"].clone();
                        if self.engine.workshop.draft == self.engine.workshop.saved_text {
                            self.engine.workshop.draft = None;
                        }
                    }
                    if !refused
                        && matches!(
                            operation.as_str(),
                            "macros.put"
                                | "macros.move"
                                | "macros.remove"
                                | "macros.new"
                                | "macros.copy"
                        )
                    {
                        self.engine.queue_workshop("catalogues", json!({}), None);
                    }
                }
                if let Some(id) = reply {
                    self.bus.reply(id, if refused { 10 } else { 0 }, body);
                }
                self.engine.workshop_settled();
                self.settle();
                continue;
            }
            if let Event::Discovered(_, snapshot) = &event
                && !self.engine.quitting
                && snapshot
                    .progress
                    .as_ref()
                    .is_some_and(|progress| progress.finished)
            {
                self.engine.queue_workshop("catalogues", json!({}), None);
            }
            if let Event::Delivery(Delivery::Theme) = event {
                self.file_theme = Theme::load();
                self.theme = self.session_theme();
                self.install_theme();
                continue;
            }
            if let Event::Look(ticket, result) = event {
                self.looked(ticket, result);
                continue;
            }
            if let Event::LookRetry(generation) = event {
                self.look_retry_due(generation);
                continue;
            }
            if let Event::Delivery(Delivery::Topic(topic)) = &event {
                for event in self.bus.take_topic_events() {
                    if event["topic"] == "citizen.topic.gap" {
                        self.engine.workshop.dropped +=
                            event["dropped"].as_u64().unwrap_or_default();
                        continue;
                    }
                    if !event["topic"].as_str().is_some_and(|topic| {
                        topic.ends_with(".command.executed") || topic.ends_with(".macros.finished")
                    }) {
                        continue;
                    }
                    let body = event["body"]
                        .as_str()
                        .and_then(|text| serde_json::from_str::<Value>(text).ok())
                        .unwrap_or(Value::Null);
                    if event["topic"]
                        .as_str()
                        .is_some_and(|topic| topic.ends_with(".macros.finished"))
                    {
                        self.engine.workshop.message = body.to_string();
                        self.engine.workshop.filter.clear();
                    }
                    self.engine.workshop.retain(json!({"ts":timestamp(),"direction":"topic","from":event["from"],"service":event["from"],"verb":event["topic"],"payload":body}));
                }
                if *topic == self.follower.topic() {
                    self.read_look();
                }
                continue;
            }
            // settingsd may have come (or come back): read the look again.
            if let Event::Delivery(Delivery::Connected | Delivery::Changed) = event {
                self.read_look();
            }
            if let Event::Delivery(Delivery::Command { id, .. }) = event
                && (self.exiting || self.engine.quitting)
            {
                self.refuse_closing(id);
                continue;
            }
            if let Event::Delivery(Delivery::Command { .. }) = event {
                self.held.push_back(event);
                self.release();
                continue;
            }
            apply_event(&mut self.engine, event);
            self.settle();
        }
        self.release();
    }

    fn refuse_closing(&self, id: u64) {
        self.bus.reply(
            id,
            10,
            json!({"error_code":"BUSY","message":label("quitting")}),
        );
    }

    /// Run held Bus commands in order. Input that only joins the drive
    /// queue goes straight on; anything else waits until the drive input
    /// before it is answered.
    fn release(&mut self) {
        while let Some(Event::Delivery(Delivery::Command { verb, .. })) = self.held.front() {
            let joins_queue = verb.strip_prefix("workshop.").is_some_and(drive::queues);
            if self.exiting || self.engine.quitting || (drive::pending(&self.ctx) && !joins_queue) {
                return;
            }
            let event = self.held.pop_front().expect("a held command");
            apply_event(&mut self.engine, event);
            self.settle();
        }
    }

    fn settle(&mut self) {
        for effect in settle(&mut self.engine, &self.commands, &self.strings) {
            self.perform(effect);
        }
        self.publish_executed();
    }

    /// Publish the commands run since the last call on
    /// `<service>.command.executed`.
    fn publish_executed(&self) {
        let (runs, lost) = self.commands.take_executed();
        if runs.is_empty() && lost == 0 {
            return;
        }
        let topic = toolkit::executed::topic(self.bus.service());
        let bodies = runs.iter().map(toolkit::executed::Executed::body).collect();
        self.bus.publish(&topic, bodies, lost);
    }

    fn perform(&mut self, effect: Effect) {
        match effect {
            Effect::Workshop {
                ticket,
                operation,
                mut args,
                reply,
            } => {
                if operation == "observe"
                    && let Some(subscription) = &self.engine.workshop.subscription
                {
                    args["subscription_id"] = json!(subscription);
                }
                if operation == "macros.run" && self.engine.workshop.subscription.is_some() {
                    let app = args["app"]
                        .as_str()
                        .filter(|app| *app != "global")
                        .unwrap_or("workshop");
                    self.engine.workshop.filter =
                        format!("macro:{app}:{}", args["id"].as_str().unwrap_or_default());
                }
                let running: std::collections::BTreeMap<String, String> = self
                    .engine
                    .snapshot
                    .services
                    .iter()
                    .filter_map(|(service, verbs)| {
                        if service.ends_with(".bus") || service == self.bus.service() {
                            return None;
                        }
                        let app = verbs
                            .as_ref()
                            .ok()?
                            .iter()
                            .find_map(|verb| verb.name.strip_suffix(".macros.list"))?;
                        if app == "workshop" {
                            return None;
                        }
                        Some((app.to_owned(), service.clone()))
                    })
                    .collect();
                let bus = self.bus.clone();
                let mut subscriptions = Vec::new();
                if operation == "catalogues" {
                    let mut topics = std::collections::BTreeSet::from([format!(
                        "{}.macros.finished",
                        self.bus.service()
                    )]);
                    for service in running.values() {
                        topics.insert(format!("{service}.command.executed"));
                        topics.insert(format!("{service}.macros.finished"));
                    }
                    subscriptions.extend(
                        self.watched_topics
                            .difference(&topics)
                            .cloned()
                            .map(|topic| (topic, false)),
                    );
                    subscriptions.extend(
                        topics
                            .difference(&self.watched_topics)
                            .cloned()
                            .map(|topic| (topic, true)),
                    );
                    self.watched_topics = topics;
                }
                self.spawn(async move {
                    for (topic, on) in subscriptions {
                        let result = if on {
                            bus.subscribe(&topic).await
                        } else {
                            bus.unsubscribe(&topic).await
                        };
                        if let Err(error) = result {
                            return Event::Workshop(ticket, operation, args, reply, Err(error));
                        }
                    }
                    let result =
                        crate::workshop::perform(bus, &operation, args.clone(), running).await;
                    Event::Workshop(ticket, operation, args, reply, result)
                });
            }
            Effect::Discover { ticket } => {
                let bus = self.bus.clone();
                let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
                self.runtime.spawn(async move {
                    bus::discover_incremental(bus, |snapshot| {
                        let _ = tx.send(Event::Discovered(ticket, snapshot));
                        ctx.request_repaint();
                    })
                    .await;
                });
            }
            Effect::Call {
                ticket,
                target,
                body,
            } => {
                let bus = self.bus.clone();
                self.spawn(async move {
                    let start = std::time::Instant::now();
                    let result = bus.raw(&target.service, &target.verb, body).await;
                    Event::Measured(
                        ticket,
                        result,
                        timestamp(),
                        start.elapsed().as_millis() as u64,
                    )
                });
            }
            Effect::Show { ticket, reply } => {
                let (bus, comp) = (self.bus.clone(), self.comp.clone());
                self.spawn(async move { Event::Shown(ticket, reply, bus::show(bus, comp).await) });
            }
            Effect::Reply { id, rc, body } => self.bus.reply(id, rc, body),
            Effect::Drive { id, verb, args } => {
                if let Some(answer) = drive::request(&self.ctx, id, &verb, &args) {
                    self.bus.reply(answer.id, answer.rc, answer.body);
                }
            }
            Effect::Copy(text) => self.ctx.copy_text(text),
            Effect::Exit => {
                if !self.macro_stopped {
                    if !self.macro_stopping {
                        self.macro_stopping = true;
                        self.spawn(async {
                            let result = tokio::task::spawn_blocking(|| {
                                macros::Session::for_app("workshop").shutdown()
                            })
                            .await
                            .map_err(|e| e.to_string())
                            .and_then(|result| result.map_err(|e| e.to_string()));
                            Event::Stopped(result)
                        });
                    }
                    return;
                }
                self.exiting = true;
                // Every accepted command gets its answer before the Bus stops:
                // drive work settled or cancelled, held commands refused.
                for answer in drive::finish(&self.ctx) {
                    self.bus.reply(answer.id, answer.rc, answer.body);
                }
                // Then every command not yet run: held, forwarded, or still in
                // the inbox, which closes so nothing more arrives.
                let mut refused: Vec<u64> = Vec::new();
                let command = |event: Event| match event {
                    Event::Delivery(Delivery::Command { id, .. }) => Some(id),
                    _ => None,
                };
                refused.extend(
                    std::mem::take(&mut self.held)
                        .into_iter()
                        .filter_map(command),
                );
                {
                    let mut inbox = locked(&self.inbox);
                    inbox.close();
                    while let Ok(delivery) = inbox.try_recv() {
                        refused.extend(command(Event::Delivery(delivery)));
                    }
                    // Forwarded ones; completions are moot once idle and quitting.
                    while let Ok(event) = self.rx.try_recv() {
                        refused.extend(command(event));
                    }
                }
                for id in refused {
                    self.refuse_closing(id);
                }
                self.bus.quit();
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            // `settle` resolves these; nothing reaches here.
            Effect::Commands { .. } | Effect::Execute { .. } | Effect::Describe { .. } => {}
        }
    }

    /// The session theme: settingsd's look when this build can draw it,
    /// else the theme file's.
    fn session_theme(&self) -> Theme {
        self.look
            .as_ref()
            .map(|look| toolkit::session_look::theme_or(look, &self.file_theme))
            .unwrap_or_else(|| self.file_theme.clone())
    }

    /// Apply the profile's bindings for Workshop over the built chords.
    pub fn set_keymap(&mut self, keymap: toolkit::keymap::Overrides) {
        if keymap == self.keymap {
            return;
        }
        for note in self.commands.rebind(&keymap) {
            eprintln!("workshop: keymap: {note}");
        }
        self.keymap = keymap;
    }

    /// Show the session look `look` (settingsd's), or the theme file's with
    /// `None`; the window's own choice stays on top. `workshop.theme`
    /// reports it as the effective session at once.
    pub fn set_look(&mut self, look: Option<settings::Appearance>) {
        self.look = look;
        self.theme = self.session_theme();
        self.install_theme();
        self.ctx.request_repaint();
    }

    /// Read the session look (start, a settingsd hint, a reconnection). A
    /// read going out voids any armed retry; closing reads nothing.
    fn read_look(&mut self) {
        if self.exiting {
            return;
        }
        if let Some(read) = self.follower.read() {
            self.perform_read(read);
        }
    }

    fn perform_read(&mut self, read: settings::follow::Read) {
        self.look_retry += 1;
        let bus = self.bus.clone();
        self.spawn(async move {
            let result = bus
                .raw_within(read.service, read.verb, read.body.to_string(), LOOK_BUDGET)
                .await;
            Event::Look(read.ticket, result)
        });
    }

    /// A look read answered: a new look is installed at once (under the
    /// window's own choice); without settingsd the theme file stands; a
    /// transient fault is retried with backoff, the last look standing.
    pub fn looked(&mut self, ticket: u64, result: Result<Reply, CallError>) {
        // Closing: nothing more to draw. A superseded read's answer changes
        // nothing, retries included.
        if self.exiting || self.follower.pending() != Some(ticket) {
            return;
        }
        let reply = result.ok();
        let answer = reply
            .as_ref()
            .map(|r| (r.rc, r.body.as_str(), r.error.as_deref()));
        let (followed, next) = self.follower.answered(ticket, answer);
        match followed {
            // A profile naming its own design source keeps the theme file,
            // as `session_look` does for one it cannot draw.
            Some(settings::follow::Followed::Look {
                appearance,
                custom_source,
                keymaps,
                ..
            }) => {
                self.set_look((!custom_source).then_some(*appearance));
                self.set_keymap(keymaps.get(APP).cloned().unwrap_or_default());
            }
            // settingsd gone: the look falls back to the theme file, but the
            // person's bindings stay in force until a profile changes them.
            Some(settings::follow::Followed::Missing) if self.look.is_some() => {
                self.set_look(None);
            }
            _ => {}
        }
        // A follow-up read goes out at once (voiding any retry); else a
        // transient fault arms one retry, backing off.
        match next {
            Some(read) => self.perform_read(read),
            None => {
                if let Some(delay) = self.follower.retry_delay() {
                    self.look_retry += 1;
                    let generation = self.look_retry;
                    self.spawn(async move {
                        tokio::time::sleep(delay).await;
                        Event::LookRetry(generation)
                    });
                }
            }
        }
    }

    /// The session look follower (tests read its retry state).
    pub fn follower(&self) -> &settings::follow::Follower {
        &self.follower
    }

    /// Ask for the session look again now (a hint, as from settingsd).
    pub fn hint_look(&mut self) {
        self.read_look();
    }

    /// The look retry armed last; a retry of any other generation is void.
    pub fn look_generation(&self) -> u64 {
        self.look_retry
    }

    /// The retry armed as `generation` is due: it reads, unless a newer
    /// read voided it or the window is closing.
    pub fn look_retry_due(&mut self, generation: u64) {
        if generation == self.look_retry && !self.exiting {
            self.read_look();
        }
    }

    /// The window's theme choice (View › Theme, the title bar's switch,
    /// `workshop.theme`).
    fn choice(&self) -> toolkit::theme_menu::Choice {
        crate::commands::choice(&self.engine)
    }

    /// Install the session theme (settingsd's look, else the theme file's)
    /// with the window's choice over it. Neither is ever written.
    fn install_theme(&mut self) {
        let choice = self.choice();
        let theme = match self.theme.with_choice(&choice) {
            Ok(theme) => theme,
            Err(error) => {
                eprintln!("toolkit: theme choice refused: {error}");
                return;
            }
        };
        toolkit::install(&self.ctx, &theme);
        self.engine.session = session_of(&self.theme);
        self.installed = choice;
    }

    /// The choice changed since the theme was installed: install again.
    fn follow_theme(&mut self) {
        if self.choice() != self.installed {
            self.install_theme();
            self.ctx.request_repaint();
        }
    }

    /// One frame's logic; it runs even while the window is hidden.
    pub fn logic(&mut self, ctx: &egui::Context) {
        // Drive answers first: a `window close` answer goes out before the
        // close it asked for is handled.
        for answer in drive::logic(ctx) {
            self.bus.reply(answer.id, answer.rc, answer.body);
        }
        self.pump();
        self.follow_theme();
        // Closing waits for accepted work: the engine quits once it is idle.
        if ctx.input(|i| i.viewport().close_requested()) && !self.exiting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.engine.close_dialog();
            self.engine.quit();
            self.settle();
        }
    }

    /// One frame's drawing and interaction.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if std::mem::take(&mut self.engine.split_requested) {
            let panel = egui::Id::new(SERVICES_PANEL);
            ui.ctx()
                .data_mut(|d| d.remove::<egui::containers::panel::PanelState>(panel));
        }
        let fired = self.commands.shortcuts(ui.ctx(), &self.engine);
        let mut events = view(ui, &self.engine, &self.commands, &self.strings);
        events.extend(
            fired
                .into_iter()
                .map(|id| UiEvent::Command(id, Origin::Shortcut)),
        );
        apply_ui(&mut self.engine, &self.commands, events);
        self.settle();
        // A command this frame may have been the last thing holding others.
        self.release();
        // A theme change is installed by the next frame's logic, before any
        // drawing: never midway through the frame that saw the click.
        if self.choice() != self.installed {
            ui.ctx().request_repaint();
        }
    }
}

impl eframe::App for Shell {
    fn on_exit(&mut self) {
        toolkit::macros::close_app(&self.ctx, "workshop");
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        Shell::logic(self, ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        Shell::ui(self, ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inspector::model::Verb;
    use inspector::{Selection, Snapshot};

    fn target(verb: &str) -> Selection {
        Selection {
            service: "example".into(),
            verb: verb.into(),
        }
    }

    fn engine() -> Engine {
        let mut s = Snapshot::default();
        let verb = |name: &str| Verb {
            name: name.into(),
            args: String::new(),
            description: String::new(),
            read_only: Some(false),
        };
        s.services
            .insert("example".into(), Ok(vec![verb("a"), verb("b")]));
        let mut e = Engine::new(label);
        let Some(Effect::Discover { ticket }) = e.take_effects().pop() else {
            panic!("initial discovery")
        };
        e.discovered(ticket, s);
        e.take_effects();
        e
    }

    fn call_targets(effects: &[Effect]) -> Vec<String> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Call { target, .. } => Some(target.verb.clone()),
                _ => None,
            })
            .collect()
    }

    /// sol finding 1: `execute(bus.call)` then `select(b)`, delivered back to
    /// back, calls `a` and refuses the select.
    #[test]
    fn an_executed_call_keeps_its_target_against_a_later_select() {
        let (commands, strings) = (crate::commands::registry(), crate::strings());
        let mut e = engine();
        e.ui.selected = Some(target("a"));
        let mut performed = Vec::new();
        for (id, verb, body) in [
            (1, "workshop.execute", r#"{"id":"bus.call"}"#),
            (2, "workshop.select", r#"{"service":"example","verb":"b"}"#),
        ] {
            apply_event(
                &mut e,
                Event::Delivery(Delivery::Command {
                    id,
                    verb: verb.into(),
                    body: body.into(),
                }),
            );
            performed.extend(settle(&mut e, &commands, &strings));
        }
        assert_eq!(call_targets(&performed), ["a"]);
        assert_eq!(e.ui.selected, Some(target("a")));
        assert!(performed.iter().any(
            |x| matches!(x, Effect::Reply { id: 2, rc: 10, body } if body["error_code"] == "BUSY")
        ));
    }

    /// sol finding 2: an edit and Ctrl+Enter in one frame call with the edit.
    #[test]
    fn a_same_frame_edit_reaches_the_call() {
        let (commands, strings) = (crate::commands::registry(), crate::strings());
        let mut e = engine();
        e.ui.selected = Some(target("a"));
        e.set_body("{\"old\":1}".into());
        let frame = vec![
            UiEvent::Command("bus.call", Origin::Ui),
            UiEvent::Body("{\"new\":2}".into()),
        ];
        apply_ui(&mut e, &commands, frame);
        let effects = settle(&mut e, &commands, &strings);
        assert!(
            matches!(&effects[..], [Effect::Call { body, .. }] if body == "{\"new\":2}"),
            "{effects:?}"
        );
        assert_eq!(e.ui.body, "{\"new\":2}");
    }

    /// The Bus and the window are one surface: filter → expand → select a
    /// row → body → call over the Bus leaves the state the same UI events
    /// leave, and sends the same call.
    #[test]
    fn bus_verbs_and_ui_events_reach_the_same_state() {
        let (commands, strings) = (crate::commands::registry(), crate::strings());
        let mut bus = engine();
        let mut performed = Vec::new();
        let steps = [
            ("workshop.filter", json!({"text":"b"})),
            ("workshop.expand", json!({"key":"mesh","open":true})),
            ("workshop.select_row", json!({"key":"verb:example:b"})),
            ("workshop.body", json!({"text":"{\"n\":1}"})),
            ("workshop.execute", json!({"id":"bus.call"})),
        ];
        for (id, (verb, body)) in (1..).zip(steps) {
            apply_event(
                &mut bus,
                Event::Delivery(Delivery::Command {
                    id,
                    verb: verb.into(),
                    body: body.to_string(),
                }),
            );
            performed.extend(settle(&mut bus, &commands, &strings));
        }
        assert!(
            performed
                .iter()
                .all(|x| !matches!(x, Effect::Reply { rc: 10, .. })),
            "{performed:?}"
        );

        let mut ui = engine();
        let row = inspector::engine::find(&ui.tree(), "verb:example:b")
            .cloned()
            .unwrap();
        let frame = vec![UiEvent::Filter("b".into()), UiEvent::Toggle("mesh".into())];
        apply_ui(&mut ui, &commands, frame);
        let frame = vec![
            UiEvent::Select(row),
            UiEvent::Body("{\"n\":1}".into()),
            UiEvent::Command("bus.call", Origin::Ui),
        ];
        apply_ui(&mut ui, &commands, frame);
        let by_hand = settle(&mut ui, &commands, &strings);

        assert_eq!(bus.ui, ui.ui);
        assert_eq!(call_targets(&performed), ["b"]);
        assert_eq!(call_targets(&performed), call_targets(&by_hand));
        let body = |effects: &[Effect]| {
            effects.iter().find_map(|e| match e {
                Effect::Call { body, .. } => Some(body.clone()),
                _ => None,
            })
        };
        assert_eq!(body(&performed), body(&by_hand));
        assert_eq!(body(&performed).as_deref(), Some("{\"n\":1}"));
    }

    #[test]
    fn every_verb_is_described_and_every_described_verb_is_handled() {
        let surface = describe();
        let names: Vec<String> = surface["verbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap().into())
            .collect();
        let mut expected: Vec<String> = model::VERBS.iter().map(|v| (*v).to_owned()).collect();
        expected.extend(
            [
                "workshop.traffic",
                "workshop.observe",
                "workshop.macros.open",
            ]
            .map(str::to_owned),
        );
        expected.extend(drive::VERBS.iter().map(|v| format!("{APP}.{v}")));
        expected.extend(
            inspector::bus::macros::describe(APP)
                .iter()
                .map(|v| v["name"].as_str().unwrap().to_owned()),
        );
        assert_eq!(names, expected);
        for name in &names {
            if name.starts_with(&format!("{APP}.macros.")) {
                continue;
            }
            let mut e = engine();
            e.command(1, name, "{}");
            let effects = settle(&mut e, &crate::commands::registry(), &crate::strings());
            assert!(
                !effects.iter().any(|x| matches!(x, Effect::Reply { body, .. } if body["error_code"] == "UNKNOWN_VERB")),
                "{name}: {effects:?}"
            );
            if let Some(verb) = name
                .strip_prefix("workshop.")
                .filter(|v| drive::VERBS.contains(v))
            {
                assert!(
                    matches!(&effects[..], [Effect::Drive { verb: v, .. }] if v == verb),
                    "{name}: {effects:?}"
                );
            }
        }
        let help = {
            let mut e = engine();
            e.command(1, "HELP", "{}");
            settle(&mut e, &crate::commands::registry(), &crate::strings())
        };
        assert!(
            matches!(&help[..], [Effect::Reply { rc: 0, body, .. }] if body == &surface["verbs"])
        );
    }

    #[test]
    fn every_command_is_reachable_by_execute() {
        let (commands, strings) = (crate::commands::registry(), crate::strings());
        for command in commands.iter() {
            let mut e = engine();
            e.ui.selected = Some(target("a"));
            if command.id.starts_with("macro.") {
                e.workshop.selected = Some(("global".into(), "test".into()));
                e.workshop.document = json!({"text":"print(1)\n"});
            }
            e.command(1, "workshop.execute", &json!({"id":command.id}).to_string());
            let effects = settle(&mut e, &commands, &strings);
            assert!(
                effects
                    .iter()
                    .any(|x| matches!(x, Effect::Reply { id: 1, rc: 0, .. })),
                "{}: {effects:?}",
                command.id
            );
        }
    }

    #[test]
    fn execute_and_commands_are_answered_in_place() {
        let (commands, strings) = (crate::commands::registry(), crate::strings());
        let mut e = engine();
        e.command(1, "workshop.commands", "{}");
        e.command(2, "workshop.execute", r#"{"id":"nope"}"#);
        e.command(3, "workshop.execute", r#"{"id":"help.about"}"#);
        e.command(4, "workshop.execute", r#"{"id":"help.about"}"#);
        let effects = settle(&mut e, &commands, &strings);
        let reply = |id: u64| {
            effects.iter().find_map(|x| match x {
                Effect::Reply { id: i, rc, body } if *i == id => Some((*rc, body.clone())),
                _ => None,
            })
        };
        let (rc, body) = reply(1).unwrap();
        assert_eq!(rc, 0);
        assert!(
            body["commands"]
                .as_array()
                .is_some_and(|c| c.iter().any(|c| c["id"] == "bus.call"))
        );
        assert_eq!(reply(2).unwrap().1["error_code"], "UNKNOWN_COMMAND");
        assert_eq!(reply(3).unwrap().0, 0);
        assert_eq!(
            reply(4).unwrap().1["error_code"],
            "DISABLED",
            "the dialog from 3 disables 4"
        );
    }
}
