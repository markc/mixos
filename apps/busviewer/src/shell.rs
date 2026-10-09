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
use crate::view::{UiEvent, view};
use crate::{label, strings};
use futures::StreamExt;
use inspector::bus::{self, CallError, Delivery, Handle, Reply};
use inspector::{Effect, Engine, Snapshot};
use serde_json::{Value, json};
use std::sync::mpsc;
use toolkit::command::CommandError;
use toolkit::{Registry, Strings, Theme, icons};

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
    let (fired, edits): (Vec<_>, Vec<_>) = events.into_iter().partition(|e| matches!(e, UiEvent::Command(_)));
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
        }
    }
}

/// Resolve the effects that only need the registry (`busviewer.commands`
/// and `busviewer.execute`) right now, in order, and return the rest for
/// the shell to perform. Running an executed command here, before any
/// later event, is what keeps its target the one selected when it arrived.
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
                        Err(CommandError::Unknown(_)) => {
                            (10, json!({"error_code":"UNKNOWN_COMMAND","message":label("unknown-command")}))
                        }
                        Err(CommandError::Disabled(_)) => {
                            (10, json!({"error_code":"DISABLED","message":label("command-disabled")}))
                        }
                    };
                    out.push(Effect::Reply { id, rc, body });
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
}

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
        let (tx, rx) = mpsc::channel();
        let forward = tx.clone();
        let wake = ctx.clone();
        runtime.spawn(async move {
            let mut deliveries = deliveries;
            while let Some(delivery) = deliveries.next().await {
                if forward.send(Event::Delivery(delivery)).is_err() {
                    return;
                }
                wake.request_repaint();
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
        };
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

    /// Apply every finished event, settling after each one.
    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            if let Event::Delivery(Delivery::Theme) = event {
                self.theme = Theme::load();
                toolkit::install(&self.ctx, &self.theme);
                continue;
            }
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
            Effect::Call { ticket, target, body } => {
                let bus = self.bus.clone();
                self.spawn(async move { Event::Completed(ticket, bus.raw(&target.service, &target.verb, body).await) });
            }
            Effect::Show { ticket, reply } => {
                let (bus, comp) = (self.bus.clone(), self.comp.clone());
                self.spawn(async move { Event::Shown(ticket, reply, bus::show(bus, comp).await) });
            }
            Effect::Reply { id, rc, body } => self.bus.reply(id, rc, body),
            Effect::Copy(text) => self.ctx.copy_text(text),
            Effect::Exit => {
                self.exiting = true;
                self.bus.quit();
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            // `settle` resolves these; nothing reaches here.
            Effect::Commands { .. } | Effect::Execute { .. } => {}
        }
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump();
        // Closing waits for accepted work: the engine quits once it is idle.
        if ctx.input(|i| i.viewport().close_requested()) && !self.exiting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.engine.close_dialog();
            self.engine.quit();
            self.settle();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let fired = self.commands.shortcuts(ui.ctx(), &self.engine);
        let stroke = icons::stroke_width(&self.theme);
        let mut events = view(ui, &self.engine, &self.commands, &self.strings, stroke);
        events.extend(fired.into_iter().map(UiEvent::Command));
        apply_ui(&mut self.engine, &self.commands, events);
        self.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inspector::model::Verb;
    use inspector::{Selection, Snapshot};

    fn target(verb: &str) -> Selection {
        Selection { service: "example".into(), verb: verb.into() }
    }

    fn engine() -> Engine {
        let mut s = Snapshot::default();
        let verb = |name: &str| Verb { name: name.into(), args: String::new(), description: String::new(), read_only: Some(false) };
        s.services.insert("example".into(), Ok(vec![verb("a"), verb("b")]));
        let mut e = Engine::new(label);
        let Some(Effect::Discover { ticket }) = e.take_effects().pop() else { panic!("initial discovery") };
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
        for (id, verb, body) in [(1, "busviewer.execute", r#"{"id":"bus.call"}"#), (2, "busviewer.select", r#"{"service":"example","verb":"b"}"#)] {
            apply_event(&mut e, Event::Delivery(Delivery::Command { id, verb: verb.into(), body: body.into() }));
            performed.extend(settle(&mut e, &commands, &strings));
        }
        assert_eq!(call_targets(&performed), ["a"]);
        assert_eq!(e.ui.selected, Some(target("a")));
        assert!(performed.iter().any(|x| matches!(x, Effect::Reply { id: 2, rc: 10, body } if body["error_code"] == "BUSY")));
    }

    /// sol finding 2: an edit and Ctrl+Enter in one frame call with the edit.
    #[test]
    fn a_same_frame_edit_reaches_the_call() {
        let (commands, strings) = (crate::commands::registry(), crate::strings());
        let mut e = engine();
        e.ui.selected = Some(target("a"));
        e.set_body("{\"old\":1}".into());
        let frame = vec![UiEvent::Command("bus.call"), UiEvent::Body("{\"new\":2}".into())];
        apply_ui(&mut e, &commands, frame);
        let effects = settle(&mut e, &commands, &strings);
        assert!(matches!(&effects[..], [Effect::Call { body, .. }] if body == "{\"new\":2}"), "{effects:?}");
        assert_eq!(e.ui.body, "{\"new\":2}");
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
        let reply = |id: u64| effects.iter().find_map(|x| match x {
            Effect::Reply { id: i, rc, body } if *i == id => Some((*rc, body.clone())),
            _ => None,
        });
        let (rc, body) = reply(1).unwrap();
        assert_eq!(rc, 0);
        assert!(body["commands"].as_array().is_some_and(|c| c.iter().any(|c| c["id"] == "bus.call")));
        assert_eq!(reply(2).unwrap().1["error_code"], "UNKNOWN_COMMAND");
        assert_eq!(reply(3).unwrap().0, 0);
        assert_eq!(reply(4).unwrap().1["error_code"], "DISABLED", "the dialog from 3 disables 4");
    }
}
