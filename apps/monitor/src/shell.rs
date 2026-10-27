// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The running window: feeds the engine its events and carries out its
//! effects. Bus work runs on a small tokio runtime; each completion is sent
//! back as an [`Event`] with a repaint request. The one clock is
//! [`INTERVAL`]: each tick asks the watched node's monitord again, and the
//! window repaints when the answers land.
//!
//! **Ordering is the contract**, as in Workshop's shell: each event is
//! applied and its effects [`settle`]d before the next is read; within a
//! frame, edits are applied before commands ([`apply_ui`]); Bus commands
//! wait behind window input accepted before them (the toolkit drive layer).
use crate::view::{UiEvent, view};
use crate::{label, strings};
use citizen::{CallError, Delivery, Handle, Reply};
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::task::Poll;
use std::time::{Duration, Instant};
use toolkit::action::{CommandError, Origin};
use toolkit::{Registry, Strings, Theme, drive};
use vitals::engine::Signal;
use vitals::model::{self, APP_ID};
use vitals::{Effect, Engine, Fetch};

/// How often the watched node is read: monitord's own sample interval.
pub const INTERVAL: Duration = Duration::from_secs(1);
/// How long one fetch may take, mesh round trip included.
const FETCH_BUDGET: Duration = Duration::from_secs(10);

/// What finished off the UI thread.
pub enum Event {
    Delivery(Delivery),
    Fetched(Fetch, u64, Result<Value, String>),
    Shown(u64, Option<u64>, Result<Value, String>),
    Signalled(u32, Signal, Result<(), String>),
    /// A read of the session look (settingsd), for the shell.
    Look(u64, Result<Reply, CallError>),
    /// A retry of the session look after a transient fault (its generation).
    LookRetry(u64),
}

/// Apply one finished event to the engine.
pub fn apply_event(engine: &mut Engine, event: Event) {
    match event {
        Event::Delivery(Delivery::Command {
            id,
            command: command_name,
            body,
        }) => engine.command(id, &command_name, &body),
        Event::Delivery(Delivery::Changed) => engine.services_changed(),
        Event::Delivery(Delivery::Connected) => engine.set_connected(true),
        Event::Delivery(Delivery::Disconnected) => engine.set_connected(false),
        // The theme and the session look are the shell's.
        Event::Delivery(Delivery::Theme | Delivery::Topic(_)) => {}
        Event::Fetched(kind, ticket, result) => engine.fetched(kind, ticket, result),
        Event::Shown(ticket, reply, result) => engine.shown(ticket, reply, result),
        Event::Signalled(pid, signal, result) => engine.signalled(pid, signal, result),
        Event::Look(..) | Event::LookRetry(_) => {}
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
            UiEvent::CloseMacros => engine.ui.macros_open = false,
            UiEvent::Command(id, origin) => {
                let _ = commands.execute(id, engine, origin);
            }
            UiEvent::Tab(tab) => engine.set_tab(tab),
            UiEvent::Section(section) => {
                let _ = engine.set_section(&section);
            }
            UiEvent::Sort(column) => engine.set_sort(column, None),
            UiEvent::Filter(filter) => engine.set_filter(filter),
            UiEvent::Select(pid) => {
                let _ = engine.select(pid);
            }
            UiEvent::Node(node) => {
                let _ = engine.set_node(node);
            }
            UiEvent::CloseDialog => engine.close_dialog(),
            UiEvent::FilterFocused => engine.filter_focused(),
            UiEvent::SetMode(mode) => engine.set_mode(mode),
        }
    }
}

/// The app prefix of Monitor's commands, and of its drive commands.
pub const APP: &str = "monitor";

/// The whole Bus surface: the engine's commands and the window's drive commands.
pub fn describe() -> Value {
    let mut surface = model::describe();
    if let Some(command_names) = surface["commands"].as_array_mut() {
        command_names.extend(drive::describe(APP));
        command_names.extend(citizen::macros::describe(APP));
    }
    surface
}

/// Resolve the effects that only need the registry or the description,
/// in order, and return the rest for the shell to perform.
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
                        Err(CommandError::Unknown(_)) => (
                            10,
                            json!({"error_code":"UNKNOWN_ACTION","message":label("unknown-action")}),
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
                        surface["commands"].clone()
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

/// A reply as the engine takes it: the body's JSON, or why there is none.
pub fn reply_value(result: Result<Reply, CallError>) -> Result<Value, String> {
    match result {
        Ok(reply) if reply.rc < 10 => serde_json::from_str(&reply.body).map_err(|e| e.to_string()),
        Ok(reply) => Err(reply.error.unwrap_or_else(|| {
            serde_json::from_str::<Value>(&reply.body)
                .ok()
                .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or(reply.body)
        })),
        Err(error) => Err(error.message),
    }
}

/// Signal one local process, as the person running Monitor.
fn send_signal(pid: u32, signal: Signal) -> Result<(), String> {
    let pid = libc::pid_t::try_from(pid)
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| "not a process id".to_owned())?;
    let number = match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    // SAFETY: kill(2) with a positive pid signals that one process.
    if unsafe { libc::kill(pid, number) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

type Deliveries = futures::channel::mpsc::Receiver<Delivery>;

fn locked(inbox: &Mutex<Deliveries>) -> std::sync::MutexGuard<'_, Deliveries> {
    inbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// This host's name: the settingsd instance to try first.
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}

/// How long one settings read may take.
const LOOK_BUDGET: Duration = Duration::from_secs(30);

fn session_of(theme: &Theme) -> vitals::Session {
    vitals::Session {
        scheme: theme.scheme(),
        style: theme.style_axis(),
        mode: theme.mode(),
        decorations: theme.requested_decorations(),
        captions: theme.captions(),
    }
}

pub struct Shell {
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
    held: VecDeque<Event>,
    inbox: Arc<Mutex<Deliveries>>,
    installed: toolkit::theme_menu::Choice,
    file_theme: Theme,
    look: Option<settings::Appearance>,
    common_ui: settings::model::CommonUi,
    keymap: toolkit::keymap::Overrides,
    follower: settings::follow::Follower,
    look_retry: u64,
    last_tick: Instant,
}

impl Shell {
    pub fn new(
        ctx: egui::Context,
        theme: Theme,
        bus: Handle,
        deliveries: Deliveries,
        comp: String,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("monitor-effects")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        drive::install(&ctx, APP);
        let (tx, rx) = mpsc::channel();
        let forward = tx.clone();
        let wake = ctx.clone();
        let inbox = Arc::new(Mutex::new(deliveries));
        let source = inbox.clone();
        runtime.spawn(async move {
            loop {
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
            engine: Engine::new(label),
            commands: crate::actions::registry(),
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
            common_ui: settings::model::CommonUi::default(),
            keymap: toolkit::keymap::Overrides::new(),
            follower: settings::follow::Follower::new(&hostname()),
            look_retry: 0,
            last_tick: Instant::now(),
        };
        shell.engine.session = session_of(&shell.theme);
        shell.settle();
        shell.read_look();
        Ok(shell)
    }

    /// Watch `node` from the start (`--node`).
    pub fn watch(&mut self, node: Option<String>) {
        let _ = self.engine.set_node(node);
        self.settle();
    }

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

    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match &event {
                Event::Delivery(citizen::Delivery::Connected) => {
                    drive::set_service(&self.ctx, Some(self.bus.service()))
                }
                Event::Delivery(citizen::Delivery::Disconnected) => {
                    drive::set_service(&self.ctx, None)
                }
                _ => {}
            }
            match event {
                Event::Delivery(Delivery::Theme) => {
                    self.file_theme = Theme::load();
                    self.theme = self.session_theme();
                    self.install_theme();
                    continue;
                }
                Event::Look(ticket, result) => {
                    self.looked(ticket, result);
                    continue;
                }
                Event::LookRetry(generation) => {
                    self.look_retry_due(generation);
                    continue;
                }
                Event::Delivery(Delivery::Topic(topic)) => {
                    if topic == self.follower.topic() {
                        self.read_look();
                    }
                    continue;
                }
                _ => {}
            }
            if let Event::Delivery(Delivery::Connected | Delivery::Changed) = event {
                self.read_look();
            }
            if let Event::Delivery(Delivery::Command { id, .. }) = event
                && self.exiting
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
        self.bus
            .reply(id, 10, json!({"error_code":"BUSY","message":label("busy")}));
    }

    fn release(&mut self) {
        while let Some(Event::Delivery(Delivery::Command {
            command: command_name,
            ..
        })) = self.held.front()
        {
            let joins_queue = command_name
                .strip_prefix("monitor.")
                .is_some_and(drive::queues);
            if self.exiting || (drive::pending(&self.ctx) && !joins_queue) {
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
            Effect::Fetch {
                ticket,
                kind,
                target,
                command: command_name,
                body,
            } => {
                let bus = self.bus.clone();
                self.spawn(async move {
                    let result = bus
                        .raw_within(&target, command_name, body.to_string(), FETCH_BUDGET)
                        .await;
                    Event::Fetched(kind, ticket, reply_value(result))
                });
            }
            Effect::Show { ticket, reply } => {
                let (bus, comp) = (self.bus.clone(), self.comp.clone());
                self.spawn(async move {
                    Event::Shown(ticket, reply, citizen::show(bus, comp, APP_ID).await)
                });
            }
            Effect::Signal { pid, signal } => {
                let result = send_signal(pid, signal);
                let _ = self.tx.send(Event::Signalled(pid, signal, result));
                self.ctx.request_repaint();
            }
            Effect::Reply { id, rc, body } => self.bus.reply(id, rc, body),
            Effect::Drive {
                id,
                command: command_name,
                args,
            } => {
                if let Some(answer) = drive::request(&self.ctx, id, &command_name, &args) {
                    self.bus.reply(answer.id, answer.rc, answer.body);
                }
            }
            Effect::Copy(text) => self.ctx.copy_text(text),
            Effect::Exit => {
                self.exiting = true;
                for answer in drive::finish(&self.ctx) {
                    self.bus.reply(answer.id, answer.rc, answer.body);
                }
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
            Effect::Commands { .. } | Effect::Execute { .. } | Effect::Describe { .. } => {}
        }
    }

    fn session_theme(&self) -> Theme {
        self.look
            .as_ref()
            .map(|look| toolkit::session_look::theme_or(look, &self.file_theme))
            .unwrap_or_else(|| self.file_theme.clone())
            .with_ui(self.common_ui.clone())
    }

    pub fn set_keymap(&mut self, keymap: toolkit::keymap::Overrides) {
        if keymap == self.keymap {
            return;
        }
        for note in self.commands.rebind(&keymap) {
            eprintln!("monitor: keymap: {note}");
        }
        self.keymap = keymap;
    }

    pub fn set_look(&mut self, look: Option<settings::Appearance>) {
        self.look = look;
        self.theme = self.session_theme();
        self.install_theme();
        self.ctx.request_repaint();
    }

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
                .raw_within(
                    read.service,
                    read.command,
                    read.body.to_string(),
                    LOOK_BUDGET,
                )
                .await;
            Event::Look(read.ticket, result)
        });
    }

    pub fn looked(&mut self, ticket: u64, result: Result<Reply, CallError>) {
        if self.exiting || self.follower.pending() != Some(ticket) {
            return;
        }
        let reply = result.ok();
        let answer = reply
            .as_ref()
            .map(|r| (r.rc, r.body.as_str(), r.error.as_deref()));
        let (followed, next) = self.follower.answered(ticket, answer);
        match followed {
            Some(settings::follow::Followed::Look {
                appearance,
                ui,
                custom_source,
                keymaps,
                ..
            }) => {
                self.common_ui = ui;
                self.set_look((!custom_source).then_some(*appearance));
                self.set_keymap(keymaps.get(APP).cloned().unwrap_or_default());
            }
            Some(settings::follow::Followed::Missing) if self.look.is_some() => {
                self.set_look(None);
            }
            _ => {}
        }
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

    pub fn look_retry_due(&mut self, generation: u64) {
        if generation == self.look_retry && !self.exiting {
            self.read_look();
        }
    }

    fn choice(&self) -> toolkit::theme_menu::Choice {
        crate::actions::choice(&self.engine)
    }

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

    fn follow_theme(&mut self) {
        if self.choice() != self.installed {
            self.install_theme();
            self.ctx.request_repaint();
        }
    }

    /// One frame's logic; it runs even while the window is hidden.
    pub fn logic(&mut self, ctx: &egui::Context) {
        drive::publish_titlebar(ctx, |topic, bodies, lost| {
            self.bus.publish(topic, bodies, lost);
        });
        for answer in drive::logic(ctx) {
            self.bus.reply(answer.id, answer.rc, answer.body);
        }
        self.pump();
        self.follow_theme();
        let since = self.last_tick.elapsed();
        if since >= INTERVAL && !self.exiting {
            self.last_tick = Instant::now();
            self.engine.tick();
            self.settle();
        }
        ctx.request_repaint_after(INTERVAL.saturating_sub(self.last_tick.elapsed()));
        if ctx.input(|i| i.viewport().close_requested()) && !self.exiting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.engine.close_dialog();
            self.engine.quit();
            self.settle();
        }
    }

    /// One frame's drawing and interaction.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let fired = self.commands.shortcuts(ui.ctx(), &self.engine);
        let mut events = view(ui, &self.engine, &self.commands, &self.strings);
        events.extend(
            fired
                .into_iter()
                .map(|id| UiEvent::Command(id, Origin::Shortcut)),
        );
        apply_ui(&mut self.engine, &self.commands, events);
        self.settle();
        self.release();
        if self.choice() != self.installed {
            ui.ctx().request_repaint();
        }
    }
}

impl eframe::App for Shell {
    fn on_exit(&mut self) {
        toolkit::macros::close_app(&self.ctx, "monitor");
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

    fn engine() -> Engine {
        let mut e = Engine::new(label);
        e.take_effects();
        e
    }

    #[test]
    fn every_command_is_described_and_every_described_command_is_handled() {
        let surface = describe();
        let names: Vec<String> = surface["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap().into())
            .collect();
        let mut expected: Vec<String> = model::COMMANDS.iter().map(|v| (*v).to_owned()).collect();
        expected.extend(drive::COMMANDS.iter().map(|v| format!("{APP}.{v}")));
        expected.extend(
            citizen::macros::describe(APP)
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
            let effects = settle(&mut e, &crate::actions::registry(), &crate::strings());
            assert!(
                !effects.iter().any(|x| matches!(x, Effect::Reply { body, .. } if body["error_code"] == "UNKNOWN_COMMAND")),
                "{name}: {effects:?}"
            );
        }
    }

    #[test]
    fn every_command_is_reachable_by_execute() {
        let (commands, strings) = (crate::actions::registry(), crate::strings());
        for command in commands.iter() {
            let mut e = engine();
            e.command(1, "monitor.execute", &json!({"id": command.id}).to_string());
            let effects = settle(&mut e, &commands, &strings);
            assert!(
                effects
                    .iter()
                    .any(|x| matches!(x, Effect::Reply { id: 1, .. })),
                "{}: {effects:?}",
                command.id
            );
        }
    }

    #[test]
    fn replies_become_values_or_reasons() {
        let ok = Reply {
            rc: 0,
            body: "{\"a\":1}".into(),
            error: None,
        };
        assert_eq!(reply_value(Ok(ok)).unwrap()["a"], 1);
        let refused = Reply {
            rc: 10,
            body: "{\"status\":\"bad_request\",\"error\":\"no\"}".into(),
            error: None,
        };
        assert_eq!(reply_value(Ok(refused)).unwrap_err(), "no");
        let broker = Reply {
            rc: 10,
            body: String::new(),
            error: Some("Service 'monitord' not found".into()),
        };
        assert!(reply_value(Ok(broker)).unwrap_err().contains("not found"));
    }

    #[test]
    fn a_group_pid_is_never_signalled() {
        assert!(send_signal(0, Signal::Term).is_err());
    }

    #[test]
    fn the_bus_and_the_window_reach_the_same_state() {
        let commands = crate::actions::registry();
        let strings = crate::strings();
        let mut bus = engine();
        for (id, (command_name, body)) in (1..).zip([
            ("monitor.tab", json!({"tab": "processes"})),
            ("monitor.sort", json!({"column": "memory"})),
            ("monitor.filter", json!({"text": "fire"})),
            ("monitor.tree", json!({"on": true})),
        ]) {
            bus.command(id, command_name, &body.to_string());
            settle(&mut bus, &commands, &strings);
        }
        let mut ui = engine();
        apply_ui(
            &mut ui,
            &commands,
            vec![
                UiEvent::Sort(vitals::Column::Memory),
                UiEvent::Filter("fire".into()),
                UiEvent::Command("view.tree", Origin::Ui),
            ],
        );
        settle(&mut ui, &commands, &strings);
        assert_eq!(bus.ui, ui.ui);
    }
}
