// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The running window: wakes from the core, the keyboard, pointer and
//! input method into the [`App`], the window's Bus verbs, the session look,
//! and the effects the window owns (clipboard, closing). Nothing polls: the
//! core's wake, the Bus and clipboard reads request a repaint, and egui
//! runs a frame.
//!
//! Ordering:
//! - Input keeps its order within a frame, and the panes are laid out
//!   before it is applied, so a click lands on the pane drawn under it.
//! - A Bus command waits while drive input the toolkit accepted before it
//!   is still unanswered (as in every MixOS app).
//! - Painting comes last: each damaged pane hands its snapshot to the GPU
//!   grid, then the frame draws.
//! - Closing answers every held command and drive request before the tab
//!   set shuts, so the lane can still send them.
//!
//! The session theme is the look settingsd holds for the desktop profile,
//! followed through a [`settings::follow::Follower`]; without settingsd the
//! theme file's stands. The window's own choice lays over either. The same
//! read carries the terminal's settings (`terminal.*`); without settingsd
//! their defaults stand. A zoom writes `terminal.font.size` back through
//! `settings.apply`, once the zoom keys rest.

use crate::app::{App, Effect, Synced};
use crate::bus::{APP, Delivery, WindowBus};
use crate::clipboard::Clipboard;
use crate::font::{Action as FontAction, Event as FontEvent, Persistence, QUIET, WriteReply};
use crate::input::{Action, Wheel};
use crate::layout;
use crate::mouse::Pointer;
use crate::view::{self, UiEvent};
use egui::Event;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;
use term_core::bus::WindowEvent;
use toolkit::command::{CommandError, Origin};
use toolkit::{Registry, Strings, Theme, drive};

type Reply = tokio::sync::oneshot::Sender<(u8, String)>;

/// A window verb waiting its turn.
struct Held {
    id: u64,
    verb: String,
    body: String,
}

/// This host's name: the settingsd instance to try first.
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}

/// The start of this process's settings operation ids: its pid and start
/// time, so a restarted term never reuses one settingsd remembers.
fn operation_prefix() -> String {
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("term-font-{}-{started}", std::process::id())
}

/// Whether a `noded.props.changed` body may mean a service (settingsd)
/// came or went.
fn services_changed(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_none_or(|b| b["path"] == "services.registered")
}

/// Events the terminal takes from egui while it has the keyboard, Alt
/// chords included: Alt is the shell's Meta.
fn takes(event: &Event) -> bool {
    matches!(
        event,
        Event::Key { .. }
            | Event::Text(_)
            | Event::Copy
            | Event::Cut
            | Event::Paste(_)
            | Event::Ime(_)
    )
}

pub struct Shell {
    pub app: App,
    commands: Registry<App>,
    strings: Strings,
    /// The session's theme (settingsd's look, else the theme file's), and
    /// the window's choice over it as installed.
    base: Theme,
    theme: Theme,
    file_theme: Theme,
    look: Option<settings::Appearance>,
    follower: settings::follow::Follower,
    look_topic: String,
    /// A read already prepared by the follower after retargeting.
    look_prepared: Option<settings::follow::Read>,
    installed: Option<toolkit::theme_menu::Choice>,
    bus: WindowBus,
    held: VecDeque<Held>,
    replies: HashMap<u64, Reply>,
    next_id: u64,
    exiting: bool,
    clipboard: Clipboard,
    ctx: egui::Context,
    /// The panes' widget: it holds the keyboard while nothing else does.
    focus: egui::Id,
    /// A zoomed font size not yet written to `terminal.font.size`; while
    /// one is, looks keep the window's size.
    font: Persistence,
    /// Names each write for settingsd's receipts: this process and a count.
    font_ops: (String, u64),
}

impl Shell {
    pub fn new(
        ctx: egui::Context,
        theme: Theme,
        app: App,
        bus: WindowBus,
        clipboard: Clipboard,
    ) -> Self {
        drive::install(&ctx, APP);
        // Ctrl+plus, minus and zero are the terminal's font size, not
        // egui's zoom.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        Self {
            app,
            commands: crate::commands::registry(),
            strings: crate::strings(),
            base: theme.clone(),
            file_theme: theme.clone(),
            theme,
            look: None,
            follower: settings::follow::Follower::new(&hostname()),
            look_topic: settings::summary_topic(settings::follow::PROFILE),
            look_prepared: None,
            installed: None,
            bus,
            held: VecDeque::new(),
            replies: HashMap::new(),
            next_id: 0,
            exiting: false,
            clipboard,
            ctx,
            focus: egui::Id::new("term.panes"),
            font: Persistence::default(),
            font_ops: (operation_prefix(), 0),
        }
    }

    pub fn commands(&self) -> &Registry<App> {
        &self.commands
    }

    /// Run `action` as its command, reached by `origin`.
    fn run(&mut self, action: Action, origin: Origin) {
        let _ = self
            .commands
            .execute(action.command(), &mut self.app, origin);
    }

    /// Apply everything the Bus and the clipboard have handed over.
    fn pump(&mut self) {
        if self.bus.start(&[
            self.look_topic.clone(),
            "theme.changed".to_owned(),
            "noded.props.changed".to_owned(),
        ]) {
            self.read_look();
        }
        while let Some(event) = self.bus.next_event() {
            match event {
                WindowEvent::Command { verb, body, reply } => {
                    if self.exiting {
                        let _ = reply.send(closing());
                        continue;
                    }
                    self.next_id += 1;
                    self.replies.insert(self.next_id, reply);
                    self.held.push_back(Held {
                        id: self.next_id,
                        verb,
                        body,
                    });
                }
                WindowEvent::Topic { topic, body } => {
                    if topic == self.look_topic {
                        // A hint, never trusted: the look comes from the read.
                        self.read_look();
                    } else if topic == "theme.changed" {
                        self.file_theme = Theme::load();
                        self.set_base();
                    } else if topic == "noded.props.changed" && services_changed(&body) {
                        self.read_look();
                    }
                }
            }
        }
        while let Some(delivery) = self.bus.next_delivery() {
            match delivery {
                Delivery::Subscribed(topic) => {
                    if topic == self.look_topic {
                        self.read_look();
                    }
                }
                Delivery::Connected => self.read_look(),
                Delivery::Look { ticket, reply } => self.looked(ticket, reply),
                Delivery::Written { reply } => self.font_written(reply),
                Delivery::FontDue => self.font_event(FontEvent::TimerFired),
            }
        }
        for (pane, read) in self.clipboard.take() {
            use crate::selection::ReadError;
            let notice = match read {
                Ok(text) => {
                    self.app.paste(pane, Some(text));
                    continue;
                }
                // Nothing to paste is not worth a notice.
                Err(ReadError::Empty) => continue,
                Err(ReadError::TooLarge) => crate::label("paste-too-large"),
                Err(ReadError::TimedOut) => crate::label("paste-timed-out"),
                Err(ReadError::Busy) => crate::label("paste-busy"),
                Err(ReadError::Failed(error)) => {
                    eprintln!("term: paste: {error}");
                    crate::label("paste-failed")
                }
            };
            self.app.paste_notice = Some(notice);
        }
        self.release();
    }

    /// Run held Bus commands in order. Drive verbs that only join the drive
    /// queue go straight on; anything else waits for drive input before it.
    fn release(&mut self) {
        while let Some(held) = self.held.front() {
            let drive_verb = held
                .verb
                .strip_prefix("term.")
                .filter(|v| drive::VERBS.contains(v))
                .map(str::to_owned);
            let joins_queue = drive_verb.as_deref().is_some_and(drive::queues);
            if self.exiting || (drive::pending(&self.ctx) && !joins_queue) {
                return;
            }
            let held = self.held.pop_front().expect("a held command");
            // The lane answered TIMEOUT already: a late command never runs.
            if self
                .replies
                .get(&held.id)
                .is_none_or(|reply| reply.is_closed())
            {
                self.replies.remove(&held.id);
                continue;
            }
            let args: Value = serde_json::from_str(&held.body).unwrap_or(Value::Null);
            match drive_verb {
                Some(verb) => {
                    if let Some(answer) = drive::request(&self.ctx, held.id, &verb, &args) {
                        self.answer(answer.id, answer.rc, answer.body);
                    }
                }
                None => {
                    let (rc, body) = self.command(&held.verb, &args);
                    self.answer(held.id, rc, body);
                    self.perform();
                }
            }
        }
    }

    /// The window's own verbs, other than drive.
    fn command(&mut self, verb: &str, args: &Value) -> (u8, Value) {
        match verb {
            "app.describe" => (0, crate::bus::describe()),
            "term.commands" => (
                0,
                json!({
                    "commands": self.commands.describe(&self.app, &self.strings),
                    "state": self.app.state(),
                }),
            ),
            "term.execute" => {
                let Some(id) = args["id"].as_str() else {
                    return (
                        10,
                        json!({"error_code":"INVALID_ARGUMENT","message":"term.execute needs {\"id\": <command id>}"}),
                    );
                };
                // A targeted command (tab.select, tab.close, pane.focus)
                // names its tab or pane; none left over for a later run.
                self.app.target_tab = args["tab"].as_u64();
                self.app.target_pane = args["pane"].as_u64();
                let run = self.commands.execute(id, &mut self.app, Origin::Bus);
                self.app.target_tab = None;
                self.app.target_pane = None;
                match run {
                    Ok(()) => (0, json!({"executed": id})),
                    Err(CommandError::Denied(_)) => (
                        10,
                        json!({"error_code":"DENIED","message":crate::label("command-denied")}),
                    ),
                    Err(CommandError::Unknown(_)) => (
                        10,
                        json!({"error_code":"UNKNOWN_COMMAND","message":crate::label("unknown-command")}),
                    ),
                    Err(CommandError::Disabled(_)) => (
                        10,
                        json!({"error_code":"DISABLED","message":crate::label("command-disabled")}),
                    ),
                }
            }
            _ => (
                10,
                json!({"error_code":"UNKNOWN_VERB","message":"unknown verb; use HELP"}),
            ),
        }
    }

    fn answer(&mut self, id: u64, rc: u8, body: Value) {
        if let Some(reply) = self.replies.remove(&id) {
            let _ = reply.send((rc, body.to_string()));
        }
    }

    /// Carry out the effects the app queued.
    fn perform(&mut self) {
        for effect in std::mem::take(&mut self.app.effects) {
            match effect {
                Effect::Write { text, primary } => self.clipboard.write(&self.ctx, text, primary),
                Effect::Read { pane, primary } => self.clipboard.read(pane, primary),
                Effect::Quit => self.close(),
            }
        }
    }

    /// Close the window: answer every held command and drive request, shut
    /// the tab set (if a quit has not already), and go.
    fn close(&mut self) {
        if self.exiting {
            return;
        }
        self.exiting = true;
        for answer in drive::finish(&self.ctx) {
            self.answer(answer.id, answer.rc, answer.body);
        }
        for held in std::mem::take(&mut self.held) {
            if let Some(reply) = self.replies.remove(&held.id) {
                let _ = reply.send(closing());
            }
        }
        for (_, reply) in self.replies.drain() {
            let _ = reply.send(closing());
        }
        self.publish_executed();
        if !self.app.is_empty() {
            self.app.quit();
            self.app.effects.clear();
        }
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// The session theme again: settingsd's look when this build can draw
    /// it, else the theme file's.
    fn set_base(&mut self) {
        self.base = self
            .look
            .as_ref()
            .and_then(toolkit::session_look::theme)
            .unwrap_or_else(|| self.file_theme.clone());
        self.installed = None;
        self.ctx.request_repaint();
    }

    fn read_look(&mut self) {
        self.font_event(FontEvent::ReadRequested);
    }

    /// A look read answered: a new look is installed at once; without
    /// settingsd the last look stands until a hint or reconnection asks again.
    fn looked(&mut self, ticket: u64, reply: Option<(u8, String, Option<String>)>) {
        if self.exiting || self.follower.pending() != Some(ticket) {
            return;
        }
        let answer = reply
            .as_ref()
            .map(|(rc, body, error)| (*rc, body.as_str(), error.as_deref()));
        let (followed, next) = self.follower.answered(ticket, answer);
        let usable = matches!(
            followed,
            Some(settings::follow::Followed::Look { .. } | settings::follow::Followed::Unchanged)
        );
        match followed {
            Some(settings::follow::Followed::Look {
                appearance,
                custom_source,
                terminal,
                ..
            }) => {
                // The terminal's settings apply whatever the look's source.
                let family = terminal.font_family(&appearance).map(str::to_owned);
                self.app
                    .apply_terminal(&terminal, family.as_deref(), self.font.pending.is_some());
                self.look = (!custom_source).then_some(*appearance);
                self.set_base();
            }
            Some(settings::follow::Followed::Missing) if self.look.is_some() => {
                self.look = None;
                self.set_base();
            }
            _ => {}
        }
        let continued = next.is_some();
        self.look_prepared = next;
        self.font_event(FontEvent::ReadAnswered {
            usable,
            following: self.follower.following(),
            continued,
        });
    }

    /// A zoom since the last frame: write its size to the settings once
    /// the zoom keys rest. Without settingsd the size stays pending, the
    /// window's own, and is written by the first look after it returns.
    fn queue_font(&mut self) {
        if std::mem::take(&mut self.app.font_reset) {
            // Back to the settings' size: an unwritten zoom is dropped. One
            // already in flight will land, so the size is written back
            // after it.
            self.font_event(FontEvent::Reset(self.app.font().current()));
        }
        let Some(px) = self.app.font_write.take() else {
            return;
        };
        self.font_event(FontEvent::Zoom(px));
    }

    /// Write the pending size, fenced on the last look read: one write in
    /// flight at a time. Without settingsd it stays pending for the next
    /// look.
    fn write_font(&mut self, px: f32) {
        self.font_ops.1 += 1;
        let id = format!("{}-{}", self.font_ops.0, self.font_ops.1);
        let changes = json!({"terminal.font.size": f64::from(px)});
        let written = self
            .follower
            .apply(&id, changes)
            .is_some_and(|write| self.bus.write_settings(write));
        if !written {
            self.font_event(FontEvent::WriteSettled(WriteReply::Failed));
        }
    }

    /// A size write answered. Changed: the look that follows carries it. A
    /// conflict (another change landed first) is written again on the next
    /// look. Failed writes wait for another zoom or successful settings read.
    /// A refusal clears the pending zoom and restores the settings' size.
    fn font_written(&mut self, reply: Option<(u8, String, Option<String>)>) {
        let status = reply
            .as_ref()
            .and_then(|(_, body, _)| serde_json::from_str::<Value>(body).ok())
            .and_then(|body| body["status"].as_str().map(str::to_owned));
        let outcome = write_outcome(status.as_deref());
        if matches!(outcome, WriteReply::Refused | WriteReply::Failed) {
            let error = reply.as_ref().and_then(|(_, _, error)| error.as_deref());
            eprintln!(
                "term: font size not saved: {}",
                status.as_deref().or(error).unwrap_or("no answer")
            );
        }
        self.font_event(FontEvent::WriteSettled(outcome));
    }

    /// Translate pure persistence actions to the native Bus and its timers.
    fn font_event(&mut self, event: FontEvent) {
        if self.exiting {
            return;
        }
        for action in self.font.event(event, Instant::now()) {
            match action {
                FontAction::Write(px) => self.write_font(px),
                FontAction::Read => {
                    if let Some(read) = self.look_prepared.take().or_else(|| self.follower.read()) {
                        let ticket = read.ticket;
                        if !self.bus.read_look(read) {
                            self.looked(ticket, None);
                        }
                    }
                }
                FontAction::Debounce => {
                    if !self.bus.font_due(QUIET) {
                        self.ctx.request_repaint_after(QUIET);
                    }
                }
            }
        }
    }

    fn follow_theme(&mut self) {
        if self.installed != Some(self.app.theme) {
            self.theme = match self.base.with_choice(&self.app.theme) {
                Ok(theme) => theme,
                Err(error) => {
                    eprintln!("toolkit: theme choice refused: {error}");
                    return;
                }
            };
            toolkit::install(&self.ctx, &self.theme);
            self.ctx.options_mut(|o| o.zoom_with_keyboard = false);
            self.installed = Some(self.app.theme);
            self.ctx.request_repaint();
        }
    }

    /// Publish the commands run since the last call on
    /// `<name>.command.executed`.
    fn publish_executed(&mut self) {
        // Commands the window performed itself (a click focusing a pane).
        for id in std::mem::take(&mut self.app.noted) {
            self.commands.note(id, Origin::Ui);
        }
        let (runs, lost) = self.commands.take_executed();
        if runs.is_empty() && lost == 0 {
            return;
        }
        let topic = toolkit::executed::topic(self.bus.name());
        let bodies = runs.iter().map(toolkit::executed::Executed::body).collect();
        self.bus.publish(&topic, bodies, lost);
    }

    /// One frame's logic; it runs even while the window is hidden.
    pub fn logic(&mut self, ctx: &egui::Context) {
        for answer in drive::logic(ctx) {
            self.answer(answer.id, answer.rc, answer.body);
        }
        self.pump();
        self.font_event(FontEvent::Now);
        if self.app.take_wake() && self.app.sync() == Synced::Exit {
            self.close();
        }
        self.app.rescale(ctx.pixels_per_point());
        self.queue_font();
        self.follow_theme();
        if ctx.input(|i| i.viewport().close_requested()) {
            // The window's close box closes every tab, as Quit does.
            self.close();
        }
        self.perform();
        self.publish_executed();
    }

    /// The window's input this frame, in order.
    fn input(&mut self, area: egui::Rect, layer: egui::LayerId, keyboard: bool) {
        let ctx = self.ctx.clone();
        let events = ctx.input_mut(|i| {
            let all = i.events.clone();
            if keyboard {
                i.events.retain(|e| !takes(e));
            }
            all
        });
        let over = |pos: egui::Pos2| area.contains(pos) && ctx.layer_id_at(pos) == Some(layer);
        let now = Instant::now();
        for event in events {
            if keyboard && matches!(event, Event::Copy | Event::Cut | Event::Paste(_)) {
                if let Some(action) = self.app.clipboard_event(&event) {
                    self.run(action, Origin::Shortcut);
                }
                continue;
            }
            match event {
                Event::ModifiersChanged(modifiers) => self.app.modifiers_changed(modifiers),
                Event::WindowFocused(true) => self.app.focused(),
                Event::WindowFocused(false) => self.app.unfocused(),
                Event::PointerMoved(pos) => self.app.pointer_event(Pointer::Moved(pos), now),
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                } => {
                    self.app.modifiers_changed(modifiers);
                    if !pressed {
                        self.app.pointer_event(Pointer::Released(pos, button), now);
                    } else if over(pos) {
                        ctx.memory_mut(|m| m.request_focus(self.focus));
                        self.app.pointer_event(Pointer::Pressed(pos, button), now);
                    } else if self.app.mouse.grabbed() {
                        // A press elsewhere ends a grab the compositor lost.
                        self.app.cancel_mouse_gesture();
                    }
                }
                Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } => {
                    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()).or(self.app.pointer)
                    else {
                        continue;
                    };
                    if over(pos) {
                        self.app.modifiers_changed(modifiers);
                        // Ctrl+wheel zooms through the zoom commands.
                        let steps = self.app.wheel_at(pos, Wheel::of(unit, delta));
                        let action = if steps > 0 {
                            Action::FontIncrease
                        } else {
                            Action::FontDecrease
                        };
                        for _ in 0..steps.unsigned_abs() {
                            self.run(action, Origin::Ui);
                        }
                    }
                }
                _ if !keyboard => {}
                Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } => {
                    self.app.modifiers_changed(modifiers);
                    if !pressed {
                        self.app.key_released(key);
                    } else if let Some(action) = self.app.key(key, modifiers, repeat) {
                        self.run(action, Origin::Shortcut);
                    }
                }
                Event::Text(text) => self.app.text(&text),
                Event::Ime(ime) => self.app.ime(&ime),
                _ => {}
            }
        }
    }

    /// One frame's drawing and interaction.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut events = view::title_bar(ui, &self.app, &self.commands, &self.strings);
        let canvas = toolkit::chrome::Chrome::of(&ctx).palette.canvas;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(canvas))
            .show(ui, |ui| {
                events.extend(view::tab_strip(ui, &self.app));
                view::notice(ui, &self.app, self.font.unsaved);
                let area = ui.available_rect_before_wrap();
                let response = ui.interact(area, self.focus, egui::Sense::click_and_drag());
                let focused = ctx.memory(|m| m.focused());
                let elsewhere = focused.is_some_and(|f| f != self.focus);
                let keyboard = !elsewhere
                    && !toolkit::menu::is_open(&ctx)
                    && !self.app.macros_open
                    && !self.app.keys_dialog
                    && !self.app.about_dialog
                    && !self.exiting;
                if keyboard && !response.has_focus() {
                    response.request_focus();
                }
                ctx.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        self.focus,
                        egui::EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: true,
                        },
                    )
                });
                self.app.layout(layout::content(area, self.app.scale()));
                self.input(area, ui.layer_id(), keyboard);
                self.app.paint();
                // A blinking cursor asks for the frame its next phase
                // needs, and nothing while it is steady or hidden.
                if let Some(delay) = self.app.next_blink(Instant::now()) {
                    ctx.request_repaint_after(delay);
                }
                view::panes(ui, &self.app);
                if keyboard
                    && self.app.keyboard_focus
                    && let Some(caret) = self.app.caret()
                {
                    // A focus change mid-composition interrupts it (ime.rs).
                    let interrupt = self.app.ime.take_interrupt();
                    ctx.output_mut(|o| {
                        o.ime = Some(egui::output::IMEOutput {
                            purpose: egui::IMEPurpose::Terminal,
                            rect: area,
                            cursor_rect: caret,
                            should_interrupt_composition: interrupt,
                        });
                    });
                }
            });
        for event in events {
            match event {
                UiEvent::Command(id, origin) => {
                    let _ = self.commands.execute(id, &mut self.app, origin);
                }
                UiEvent::SelectTab(id) => {
                    self.app.target_tab = Some(id);
                    let _ = self
                        .commands
                        .execute("tab.select", &mut self.app, Origin::Ui);
                }
                UiEvent::CloseTab(id) => {
                    self.app.target_tab = Some(id);
                    let _ = self
                        .commands
                        .execute("tab.close", &mut self.app, Origin::Ui);
                }
            }
        }
        if toolkit::macros::show_app(&ctx, "term", self.app.macros_open) {
            self.app.macros_open = false;
        }
        if self.app.keys_dialog && view::keys_dialog(&ctx, &self.commands, &self.strings) {
            self.app.keys_dialog = false;
        }
        if self.app.about_dialog
            && view::about_dialog(&ctx, self.bus.connected() && self.follower.following())
        {
            self.app.about_dialog = false;
        }
        self.queue_font();
        self.perform();
        self.release();
        self.publish_executed();
    }
}

fn closing() -> (u8, String) {
    (
        10,
        json!({"error_code":"BUSY","message":"term is closing"}).to_string(),
    )
}

fn write_outcome(status: Option<&str>) -> WriteReply {
    match status {
        Some("changed" | "unchanged") => WriteReply::Saved,
        Some("conflict") => WriteReply::Conflict,
        Some("validation_failed" | "invalid_request") => WriteReply::Refused,
        _ => WriteReply::Failed,
    }
}

impl eframe::App for Shell {
    fn on_exit(&mut self) {
        toolkit::macros::close_app(&self.ctx, "term");
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

    #[test]
    fn write_answers_distinguish_failures_and_validation_refusals() {
        for status in [None, Some("storage_failed"), Some("outcome_unknown")] {
            assert_eq!(write_outcome(status), WriteReply::Failed);
        }
        for status in ["validation_failed", "invalid_request"] {
            assert_eq!(write_outcome(Some(status)), WriteReply::Refused);
        }
        assert_eq!(write_outcome(Some("conflict")), WriteReply::Conflict);
        for status in ["changed", "unchanged"] {
            assert_eq!(write_outcome(Some(status)), WriteReply::Saved);
        }
    }
}
