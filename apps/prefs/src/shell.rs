// SPDX-License-Identifier: MIT OR Apache-2.0
//! The running window: feeds the engine its events and carries out its
//! effects. Bus work runs on a small tokio runtime; each completion is sent
//! back as an [`Event`] with a repaint request, so the window wakes only
//! when something happened (no polling).
//!
//! **Ordering is the contract** (as in BusViewer). Each event is applied and
//! its effects are [`settle`]d before the next event is read, so a later
//! delivery can never change what an earlier command acts on. Within a
//! frame, selections are applied before commands ([`apply_ui`]), so a
//! command acts on the row the person sees selected. Bus commands also wait
//! behind window input accepted before them (the toolkit drive layer); and
//! every accepted command is answered before the Bus stops.
use crate::view::{UiEvent, view};
use crate::{label, strings};
use citizen::{CallError, Delivery, Handle, Reply};
use futures::StreamExt;
use preferences::appearance::SETTINGS;
use preferences::model::{self, RELEASES};
use preferences::{Effect, Engine, Look};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::task::Poll;
use std::time::Duration;
use toolkit::command::CommandError;
use toolkit::{Registry, Strings, Theme, drive, icons};

/// What finished off the UI thread.
pub enum Event {
    Delivery(Delivery),
    Released(u64, Result<Reply, CallError>),
    Settled(u64, Result<Reply, CallError>),
    Shown(u64, Option<u64>, Result<Value, String>),
}

/// Apply one finished event to the engine.
pub fn apply_event(engine: &mut Engine, event: Event) {
    match event {
        Event::Delivery(delivery) => engine.delivery(delivery),
        Event::Released(ticket, result) => engine.released(ticket, result),
        Event::Settled(ticket, result) => engine.settled(ticket, result),
        Event::Shown(ticket, reply, result) => engine.shown(ticket, reply, result),
    }
}

/// Apply one frame's interactions: selections and dialog answers first,
/// then commands, so a command acts on what was selected this frame.
pub fn apply_ui(engine: &mut Engine, commands: &Registry<Engine>, events: Vec<UiEvent>) {
    let (fired, edits): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|e| matches!(e, UiEvent::Command(_)));
    for event in edits.into_iter().chain(fired) {
        match event {
            UiEvent::Command(id) => {
                let _ = commands.execute(id, engine);
            }
            UiEvent::Select(app) => engine.select(&app),
            UiEvent::CloseDialog => engine.close_dialog(),
            UiEvent::ConfirmRemove => engine.confirm_remove(),
            UiEvent::SetMode(mode) => engine.set_mode(mode),
            UiEvent::Look(look) => engine.edit_look(look),
        }
    }
}

/// The app prefix of Prefs' verbs, and of its drive verbs.
pub const APP: &str = "prefs";

/// How long a releases call may take: reads are quick, a check asks GitHub
/// once per app, and a change downloads and unpacks (releasesd bounds each
/// file at 30 minutes; an update covers every installed app).
pub fn budget(verb: &str) -> Duration {
    Duration::from_secs(match verb {
        "releases.list" | "releases.notes" | "releases.status" => 30,
        "releases.check" => 600,
        _ => 4 * 3600,
    })
}

/// How long a settingsd call may take: every verb answers from memory or
/// one durable write.
pub const SETTINGS_BUDGET: Duration = Duration::from_secs(30);

/// This host's name: the settingsd instance to try first (settingsd runs
/// as `--instance %H`; the panel adopts the right one from its answer).
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "host".into())
}

/// The whole Bus surface: the engine's verbs and the window's drive verbs.
pub fn describe() -> Value {
    let mut surface = model::describe();
    if let Some(verbs) = surface["verbs"].as_array_mut() {
        verbs.extend(drive::describe(APP));
    }
    surface
}

/// Resolve the effects that only need the registry (`prefs.commands` and
/// `prefs.execute`) or the description right now, in order, and return the
/// rest for the shell to perform.
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

/// The look the window previews: the Appearance draft, else settingsd's
/// look, unless the profile uses a design package of its own (then the
/// session theme, which carries it).
pub fn base_look(engine: &Engine) -> Option<Look> {
    if engine.look.custom_source {
        None
    } else {
        engine.look.shown()
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
    /// The Bus deliveries not yet forwarded (see BusViewer's shell: shutdown
    /// closes and drains it under its lock).
    inbox: Arc<Mutex<Deliveries>>,
    /// The window choice and base look installed last.
    installed: Installed,
}

type Deliveries = futures::channel::mpsc::Receiver<Delivery>;

/// The window's own theme choice and the look under it.
type Installed = (toolkit::theme_menu::Choice, Option<Look>);

fn locked(inbox: &Mutex<Deliveries>) -> std::sync::MutexGuard<'_, Deliveries> {
    inbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Shell {
    /// A window on `bus`, with its deliveries forwarded from `deliveries`.
    pub fn new(
        ctx: egui::Context,
        theme: Theme,
        bus: Handle,
        deliveries: Deliveries,
        comp: String,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("prefs-effects")
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
            engine: Engine::new(crate::label_with),
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
            installed: (toolkit::theme_menu::Choice::default(), None),
        };
        shell.engine.session = (shell.theme.scheme(), shell.theme.mode());
        shell.engine.start_appearance(&hostname());
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
            let joins_queue = verb.strip_prefix("prefs.").is_some_and(drive::queues);
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
            Effect::Releases { ticket, verb, body } => {
                let bus = self.bus.clone();
                self.spawn(async move {
                    let result = bus
                        .raw_within(RELEASES, verb, body.to_string(), budget(verb))
                        .await;
                    Event::Released(ticket, result)
                });
            }
            Effect::Settings { ticket, verb, body } => {
                let bus = self.bus.clone();
                self.spawn(async move {
                    let result = bus
                        .raw_within(SETTINGS, verb, body.to_string(), SETTINGS_BUDGET)
                        .await;
                    Event::Settled(ticket, result)
                });
            }
            Effect::Show { ticket, reply } => {
                let (bus, comp) = (self.bus.clone(), self.comp.clone());
                self.spawn(async move {
                    Event::Shown(ticket, reply, citizen::show(bus, comp, model::APP_ID).await)
                });
            }
            Effect::Reply { id, rc, body } => self.bus.reply(id, rc, body),
            Effect::Drive { id, verb, args } => {
                if let Some(answer) = drive::request(&self.ctx, id, &verb, &args) {
                    self.bus.reply(answer.id, answer.rc, answer.body);
                }
            }
            Effect::Exit => {
                self.exiting = true;
                // Every accepted command gets its answer before the Bus stops:
                // drive work settled or cancelled, held commands refused.
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
            // `settle` resolves these; nothing reaches here.
            Effect::Commands { .. } | Effect::Execute { .. } | Effect::Describe { .. } => {}
        }
    }

    fn choice(&self) -> toolkit::theme_menu::Choice {
        crate::commands::choice(&self.engine)
    }

    /// What the window is drawn in, under its own choice: the Appearance
    /// draft or settingsd's look (embedded design), else the session theme.
    fn wanted(&self) -> Installed {
        (self.choice(), base_look(&self.engine))
    }

    /// Install the base theme with the window's choice over it. Neither the
    /// session's theme file nor settingsd is written.
    fn install_theme(&mut self) {
        let (choice, look) = self.wanted();
        let base = match look {
            Some(look) => {
                Theme::for_context(look.context()).framed(look.decorations, look.captions)
            }
            None => self.theme.clone(),
        };
        toolkit::install(&self.ctx, &base.with_choice(&choice));
        self.engine.session = (base.scheme(), base.mode());
        self.installed = (choice, look);
    }

    fn follow_theme(&mut self) {
        if self.wanted() != self.installed {
            self.install_theme();
            self.ctx.request_repaint();
        }
    }

    /// One frame's logic; it runs even while the window is hidden.
    pub fn logic(&mut self, ctx: &egui::Context) {
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
        let fired = self.commands.shortcuts(ui.ctx(), &self.engine);
        let stroke = icons::stroke_width(&self.theme);
        let mut events = view(ui, &self.engine, &self.commands, &self.strings, stroke);
        events.extend(fired.into_iter().map(UiEvent::Command));
        apply_ui(&mut self.engine, &self.commands, events);
        self.settle();
        self.release();
        if self.wanted() != self.installed {
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

    fn engine() -> Engine {
        let mut e = Engine::new(crate::label_with);
        let ticket = match e.take_effects().pop() {
            Some(Effect::Releases { ticket, .. }) => ticket,
            other => panic!("initial listing, got {other:?}"),
        };
        let rows = json!([
            {"app":"demo","repo":"o/demo","installed":"1.0","latest":"2.0","published":"2026-10-10","status":"update"}
        ]);
        e.released(
            ticket,
            Ok(Reply {
                rc: 0,
                body: rows.to_string(),
                error: None,
            }),
        );
        e.take_effects();
        e
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
                .strip_prefix("prefs.")
                .filter(|v| drive::VERBS.contains(v))
            {
                assert!(
                    matches!(&effects[..], [Effect::Drive { verb: v, .. }] if v == verb),
                    "{name}: {effects:?}"
                );
            }
        }
    }

    #[test]
    fn every_command_is_reachable_by_execute() {
        let (commands, strings) = (crate::commands::registry(), crate::strings());
        for command in commands.iter() {
            let mut e = engine();
            e.command(1, "prefs.execute", &json!({"id":command.id}).to_string());
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

    /// A same-frame selection reaches the command fired with it: Install
    /// acts on the row clicked in the same frame.
    #[test]
    fn a_same_frame_selection_reaches_the_command() {
        let (commands, strings) = (crate::commands::registry(), crate::strings());
        let mut e = engine();
        let frame = vec![
            UiEvent::Command("apps.install"),
            UiEvent::Select("demo".into()),
        ];
        apply_ui(&mut e, &commands, frame);
        let effects = settle(&mut e, &commands, &strings);
        let verbs: Vec<_> = effects
            .iter()
            .filter_map(|x| match x {
                Effect::Releases { verb, .. } => Some(*verb),
                _ => None,
            })
            .collect();
        assert_eq!(verbs, ["releases.notes", "releases.update"]);
        assert!(effects.iter().any(|x| matches!(x, Effect::Releases { verb: "releases.update", body, .. } if body["apps"][0] == "demo")));
    }

    #[test]
    fn long_operations_get_long_budgets() {
        assert_eq!(budget("releases.list"), Duration::from_secs(30));
        assert!(budget("releases.install") >= Duration::from_secs(3600));
        assert!(budget("releases.update") >= budget("releases.install"));
    }
}
