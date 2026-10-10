// SPDX-License-Identifier: MIT OR Apache-2.0
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
//! toolkit drive layer), so `ui.type` then `busviewer.call` calls with the
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
use toolkit::command::CommandError;
use toolkit::{Registry, Strings, Theme, drive, icons};

/// What finished off the UI thread.
pub enum Event {
    Delivery(Delivery),
    Discovered(u64, Snapshot),
    Completed(u64, Result<Reply, CallError>),
    Shown(u64, Option<u64>, Result<Value, String>),
}

/// Apply one finished event to the engine.
pub fn apply_event(engine: &mut Engine, event: Event) {
    match event {
        Event::Delivery(delivery) => engine.delivery(delivery),
        Event::Discovered(ticket, snapshot) => engine.discovered(ticket, snapshot),
        Event::Completed(ticket, result) => engine.completed(ticket, result),
        Event::Shown(ticket, reply, result) => engine.shown(ticket, reply, result),
    }
}

/// Apply one frame's interactions: edits first, then commands, so a command
/// sees every edit made in the same frame.
pub fn apply_ui(engine: &mut Engine, commands: &Registry<Engine>, events: Vec<UiEvent>) {
    let (fired, edits): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|e| matches!(e, UiEvent::Command(_)));
    for event in edits.into_iter().chain(fired) {
        match event {
            UiEvent::Command(id) => {
                let _ = commands.execute(id, engine);
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

/// The app prefix of BusViewer's verbs, and of its drive verbs.
pub const APP: &str = "busviewer";

/// The whole Bus surface: the engine's verbs and the window's drive verbs.
pub fn describe() -> Value {
    let mut surface = model::describe();
    if let Some(verbs) = surface["verbs"].as_array_mut() {
        verbs.extend(drive::describe(APP));
    }
    surface
}

/// Resolve the effects that only need the registry (`busviewer.commands`
/// and `busviewer.execute`) or the description right now, in order, and
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
                    let (rc, body) = match commands.execute(&command, engine) {
                        Ok(()) => (0, json!({ "executed": command })),
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
}

/// What the engine shows for an unchosen axis: the session theme's.
fn session_of(theme: &Theme) -> inspector::Session {
    inspector::Session {
        scheme: theme.scheme(),
        style: theme.style_axis(),
        mode: theme.mode(),
        decorations: theme.decorations(),
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
            .thread_name("busviewer-effects")
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
            engine: Engine::new(label),
            commands: crate::commands::registry(),
            strings: strings(),
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
        };
        shell.engine.session = session_of(&shell.theme);
        shell.settle();
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

    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// Apply every finished event, settling after each one. A Bus command
    /// is held, in order, while drive input accepted before it is still
    /// unanswered; completions are applied at once.
    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            if let Event::Delivery(Delivery::Theme) = event {
                self.theme = Theme::load();
                self.install_theme();
                continue;
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
            let joins_queue = verb.strip_prefix("busviewer.").is_some_and(drive::queues);
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
    }

    fn perform(&mut self, effect: Effect) {
        match effect {
            Effect::Discover { ticket } => {
                let bus = self.bus.clone();
                self.spawn(async move { Event::Discovered(ticket, bus::discover(bus).await) });
            }
            Effect::Call {
                ticket,
                target,
                body,
            } => {
                let bus = self.bus.clone();
                self.spawn(async move {
                    Event::Completed(ticket, bus.raw(&target.service, &target.verb, body).await)
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

    /// The window's theme choice (View › Theme, the title bar's switch,
    /// `busviewer.theme`).
    fn choice(&self) -> toolkit::theme_menu::Choice {
        crate::commands::choice(&self.engine)
    }

    /// Install the session theme with the window's choice over it. The
    /// session's theme file is never written.
    fn install_theme(&mut self) {
        let choice = self.choice();
        toolkit::install(&self.ctx, &self.theme.with_choice(&choice));
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
        let stroke = icons::stroke_width(&self.theme);
        let mut events = view(ui, &self.engine, &self.commands, &self.strings, stroke);
        events.extend(fired.into_iter().map(UiEvent::Command));
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
            (1, "busviewer.execute", r#"{"id":"bus.call"}"#),
            (2, "busviewer.select", r#"{"service":"example","verb":"b"}"#),
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
            UiEvent::Command("bus.call"),
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
            ("busviewer.filter", json!({"text":"b"})),
            ("busviewer.expand", json!({"key":"mesh","open":true})),
            ("busviewer.select_row", json!({"key":"verb:example:b"})),
            ("busviewer.body", json!({"text":"{\"n\":1}"})),
            ("busviewer.execute", json!({"id":"bus.call"})),
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
            UiEvent::Command("bus.call"),
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
        expected.extend(drive::VERBS.iter().map(|v| format!("{APP}.{v}")));
        assert_eq!(names, expected);
        for name in &names {
            let mut e = engine();
            e.command(1, name, "{}");
            let effects = settle(&mut e, &crate::commands::registry(), &crate::strings());
            assert!(
                !effects.iter().any(|x| matches!(x, Effect::Reply { body, .. } if body["error_code"] == "UNKNOWN_VERB")),
                "{name}: {effects:?}"
            );
            if let Some(verb) = name
                .strip_prefix("busviewer.")
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
            e.command(
                1,
                "busviewer.execute",
                &json!({"id":command.id}).to_string(),
            );
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
        e.command(1, "busviewer.commands", "{}");
        e.command(2, "busviewer.execute", r#"{"id":"nope"}"#);
        e.command(3, "busviewer.execute", r#"{"id":"help.about"}"#);
        e.command(4, "busviewer.execute", r#"{"id":"help.about"}"#);
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
