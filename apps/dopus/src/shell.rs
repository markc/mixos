// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The running window: feeds the core its events, serves Bus commands, and
//! performs the effects the app leaves (replies, opening files, the theme,
//! quitting). Nothing polls:
//! - a pump thread forwards the core's channel and wakes the window (law 2:
//!   every event goes through `on_event` once, here, on the UI thread);
//! - the core is ticked after each frame's work, and the window asks for a
//!   frame at the core's next deadline (law 1);
//! - the Bus thread wakes the window after each delivery.
//!
//! Ordering: deliveries are applied in arrival order, each with its effects
//! performed before the next. A Bus command waits while drive input the
//! toolkit accepted before it is still unanswered, so a `dopus.execute`
//! never overtakes a `dopus.ui.click` sent first. Quitting answers every
//! dialog No, refuses held commands, saves the layout and stops the Bus;
//! the window closes when the Bus has stopped.
//!
//! The session theme is the look settingsd holds for the desktop profile,
//! followed through a [`settings::follow::Follower`]: read at start, on a
//! hint on the profile's summary topic, on a reconnection and when services
//! register. Without settingsd, or with a look this build cannot draw, the
//! theme file's session theme stands (and follows `theme.changed`). The
//! window's own choice lays over either.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dopus_core::CoreEvent;
use serde_json::Value;
use toolkit::{Registry, Strings, Theme, drive};

use crate::app::{App, Effect};
use crate::bus::{BusHandle, Delivery};
use crate::commands::{self, ActionReply, AppearanceState, Command, ServerMeta, ThemeAction};
use crate::view::{apply, view};

/// The app prefix of dopus's commands, and of its drive commands.
pub const APP: &str = "dopus";

/// The topic the theme file's changes are announced on.
const THEME_TOPIC: &str = "theme.changed";
/// noded's property changes: settingsd registering among them.
const PROPS_TOPIC: &str = "noded.props.changed";

/// This host's name: the settingsd instance to try first.
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}

/// Whether a `noded.props.changed` body may mean a service came or went.
fn services_changed(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_none_or(|b| b["path"] == "services.registered")
}

/// The Bus, when there is one.
pub struct Bus {
    pub handle: BusHandle,
    pub deliveries: futures::channel::mpsc::UnboundedReceiver<Delivery>,
}

pub struct Shell {
    pub app: App,
    commands: Registry<App>,
    strings: Strings,
    /// The session theme (settingsd's look, else the theme file's), and the
    /// window's choice over it as installed.
    base: Theme,
    theme: Theme,
    file_theme: Theme,
    look: Option<settings::view::Look>,
    custom_source: bool,
    common_ui: settings::model::CommonUi,
    /// The profile's bindings for dopus, as last applied.
    keymap: toolkit::keymap::Overrides,
    follower: settings::follow::Follower,
    look_retry: u64,
    installed: Option<toolkit::theme_menu::Choice>,
    bus: Option<Bus>,
    core_events: mpsc::Receiver<CoreEvent>,
    ctx: egui::Context,
    service: String,
    config_path: Option<String>,
    /// Bus commands waiting behind drive input accepted before them.
    held: VecDeque<Command>,
    /// Input after a shortcut that followed a click in the same frame,
    /// waiting for the next frame ([`Shell::split_input`]).
    deferred: Vec<egui::Event>,
    exiting: bool,
    /// The window title last sent.
    title: String,
}

impl Shell {
    /// A window around `app`. `core_events` is the core's channel; a pump
    /// thread forwards it and wakes the window.
    pub fn new(
        ctx: egui::Context,
        theme: Theme,
        app: App,
        core_events: mpsc::Receiver<CoreEvent>,
        bus: Option<Bus>,
        service: String,
        config_path: Option<String>,
    ) -> Self {
        drive::install(&ctx, APP);
        let (tx, rx) = mpsc::channel();
        let wake = ctx.clone();
        let pumped = std::thread::Builder::new()
            .name("dopus-core-events".into())
            .spawn(move || {
                for event in core_events {
                    if tx.send(event).is_err() {
                        break;
                    }
                    wake.request_repaint();
                }
            });
        if let Err(error) = pumped {
            eprintln!("dopus: core event pump: {error}");
        }
        let commands = crate::actions::registry();
        let shell = Self {
            app,
            commands,
            strings: crate::strings(),
            base: theme.clone(),
            file_theme: theme.clone(),
            theme,
            look: None,
            custom_source: false,
            common_ui: settings::model::CommonUi::default(),
            keymap: toolkit::keymap::Overrides::new(),
            follower: settings::follow::Follower::new(&hostname()),
            look_retry: 0,
            installed: None,
            bus,
            core_events: rx,
            ctx,
            service,
            config_path,
            held: VecDeque::new(),
            deferred: Vec::new(),
            exiting: false,
            title: String::new(),
        };
        if let Some(bus) = &shell.bus {
            // Hints arrive once the subscription lands; the read then
            // (`Delivery::Subscribed`) catches any change missed before.
            for topic in shell
                .follower
                .topics()
                .into_iter()
                .chain([THEME_TOPIC.into(), PROPS_TOPIC.into()])
            {
                bus.handle.subscribe(topic);
            }
        }
        shell
    }

    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }

    fn meta(&self) -> ServerMeta {
        ServerMeta {
            service: self.service.clone(),
            headless: false,
            window_available: self.app.window_available(),
            config_path: self.config_path.clone(),
            theme_scheme: self.theme.scheme().name().to_owned(),
            theme_mode: self.theme.mode().name().to_owned(),
            appearance: AppearanceState {
                icons: "lucide".into(),
                asset_set: None,
                font_ui: "Inter".into(),
                font_mono: "JetBrains Mono".into(),
                font_ui_weight: 400,
                icon_weight: None,
            },
            actions: self.app.actions.clone(),
            extra_commands: drive::COMMANDS
                .iter()
                .map(|v| format!("{APP}.{v}"))
                .collect(),
        }
    }

    /// Feed the core every event that has arrived (law 2), then tick it
    /// (law 1); dialogs, opens and status lines go to the app.
    fn pump_core(&mut self) {
        while let Ok(event) = self.core_events.try_recv() {
            let derived = self.app.core.on_event(event);
            self.app.absorb(derived);
        }
        let derived = self.app.core.tick(Instant::now());
        self.app.absorb(derived);
    }

    /// Apply every Bus delivery that has arrived.
    fn pump_bus(&mut self) {
        drive::set_service(
            &self.ctx,
            self.bus
                .as_ref()
                .filter(|bus| bus.handle.connected())
                .map(|bus| bus.handle.service()),
        );
        while let Some(delivery) = self.bus.as_mut().and_then(|b| b.deliveries.try_recv().ok()) {
            match delivery {
                Delivery::Mail { id, result } => self.app.mailed(id, result),
                Delivery::Command(cmd) => {
                    if self.exiting {
                        self.refuse_closing(cmd.id);
                    } else {
                        self.held.push_back(cmd);
                        self.release();
                    }
                }
                Delivery::Topic { topic, body } => {
                    if self.follower.is_hint(&topic) {
                        // A hint, never trusted: the look comes from the read.
                        self.read_look();
                    } else if topic == THEME_TOPIC {
                        self.file_theme = Theme::load();
                        self.set_base();
                    } else if topic == PROPS_TOPIC && services_changed(&body) {
                        self.read_look();
                    }
                }
                Delivery::Connected => self.read_look(),
                Delivery::Disconnected => {}
                Delivery::Subscribed { topic } => {
                    if self.follower.is_hint(&topic) {
                        self.read_look();
                    }
                }
                Delivery::Look { ticket, reply } => self.looked(ticket, reply),
                Delivery::LookRetry { generation } => {
                    if generation == self.look_retry && !self.exiting {
                        self.read_look();
                    }
                }
                Delivery::Stopped => {
                    self.exiting = true;
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            self.perform();
        }
        self.release();
    }

    /// Run held Bus commands in order. Drive commands that only join the drive
    /// queue go straight on; anything else waits for drive input before it.
    fn release(&mut self) {
        while let Some(cmd) = self.held.front() {
            let drive_command = cmd
                .command
                .strip_prefix("dopus.")
                .filter(|v| drive::COMMANDS.contains(v))
                .map(str::to_owned);
            let joins_queue = drive_command.as_deref().is_some_and(drive::queues);
            if self.exiting || (drive::pending(&self.ctx) && !joins_queue) {
                return;
            }
            let cmd = self.held.pop_front().expect("a held command");
            match drive_command {
                Some(command_name) => {
                    let args: Value = serde_json::from_str(&cmd.body).unwrap_or(Value::Null);
                    if let Some(answer) = drive::request(&self.ctx, cmd.id, &command_name, &args) {
                        self.respond(answer.id, answer.rc, answer.body.to_string());
                    }
                }
                None => {
                    let meta = self.meta();
                    self.app.serve(&cmd, &meta);
                    self.perform();
                    self.note_bus_runs();
                }
            }
        }
    }

    fn respond(&self, id: u64, rc: u8, body: String) {
        if let Some(bus) = &self.bus {
            bus.handle.respond(id, rc, body);
        }
    }

    fn refuse_closing(&self, id: u64) {
        let body = r#"{"error_code":"BUSY","message":"dopus is closing"}"#.to_owned();
        self.respond(id, 10, body);
    }

    /// Perform the effects the app left. A quit finishes its batch first,
    /// so its own answer goes out.
    fn perform(&mut self) {
        let mut quit = false;
        while !self.app.effects.is_empty() {
            for effect in std::mem::take(&mut self.app.effects) {
                match effect {
                    Effect::Mail { id, paths } => {
                        if !self
                            .bus
                            .as_ref()
                            .is_some_and(|bus| bus.handle.mail(id, paths))
                        {
                            self.app
                                .mailed(id, Err(crate::bus::MailFailure::unavailable()));
                        }
                    }
                    Effect::Reply { id, rc, body } => self.respond(id, rc, body),
                    Effect::OpenFile(path) => self.open_file(path),
                    Effect::ThemeSet { id, scheme, mode } => self.theme_set(id, scheme, mode),
                    Effect::ThemeAction { id, action, name } => {
                        self.theme_action(action);
                        if let Some(id) = id {
                            let body = serde_json::to_string(&ActionReply {
                                id: name,
                                ok: true,
                                result: None,
                            })
                            .unwrap_or_default();
                            self.respond(id, 0, body);
                        }
                    }
                    Effect::Show { id } => {
                        if !self.bus.as_ref().is_some_and(|bus| bus.handle.show(id)) {
                            let body =
                                r#"{"error_code":"ACTIVATION","message":"the Bus is not running"}"#;
                            self.respond(id, 10, body.to_owned());
                        }
                    }
                    Effect::Quit => quit = true,
                }
            }
        }
        if quit {
            self.quit();
        }
    }

    /// Law 4: open a file with the desktop's handler, detached. Only a
    /// failure to start it is reported.
    fn open_file(&mut self, path: std::path::PathBuf) {
        match std::process::Command::new("xdg-open").arg(&path).spawn() {
            Ok(mut child) => {
                // Reap it off the UI thread, so it never lingers as a zombie.
                let reaped = std::thread::Builder::new()
                    .name("dopus-xdg-open-reap".into())
                    .spawn(move || drop(child.wait()));
                if reaped.is_err() {
                    tracing::warn!("dopus: xdg-open reaper did not start");
                }
            }
            Err(error) => {
                self.app.status = Some(format!(
                    "Opening {}: {error}",
                    dopus_core::sanitise_display_path(&path)
                ));
            }
        }
    }

    /// `dopus.theme.set`: the window's scheme and mode by name; the reply
    /// names what the window then shows.
    fn theme_set(&mut self, id: Option<u64>, scheme: Option<String>, mode: Option<String>) {
        let mut refusal = None;
        if let Some(name) = scheme {
            match design::Scheme::from_name(&name) {
                Some(s) => self.app.theme.scheme = Some(s),
                None => refusal = Some(format!("unknown scheme {name:?}")),
            }
        }
        if let Some(name) = mode {
            match design::Mode::from_name(&name) {
                Some(m) => self.app.theme.mode = Some(m),
                None => refusal = Some(format!("unknown mode {name:?}")),
            }
        }
        self.follow_theme();
        let Some(id) = id else { return };
        match refusal {
            Some(message) => {
                let body = serde_json::to_string(&commands::Refusal {
                    error_code: commands::code::INVALID_ARGUMENT.into(),
                    message,
                    reason: None,
                })
                .unwrap_or_default();
                self.respond(id, 10, body);
            }
            None => {
                let body = serde_json::to_string(&commands::ThemeSetReply {
                    scheme: self.theme.scheme().name().into(),
                    mode: self.theme.mode().name().into(),
                })
                .unwrap_or_default();
                self.respond(id, 0, body);
            }
        }
    }

    fn theme_action(&mut self, action: ThemeAction) {
        match action {
            ThemeAction::Scheme(name) => {
                if let Some(s) = design::Scheme::from_name(&name) {
                    self.app.theme.scheme = Some(s);
                }
            }
            ThemeAction::ModeToggle => {
                // The mode opposite the one shown.
                self.app.theme.mode = Some(match self.theme.mode() {
                    design::Mode::Dark => design::Mode::Light,
                    design::Mode::Light => design::Mode::Dark,
                });
            }
        }
        self.follow_theme();
    }

    /// Stop: answer every dialog No (law 3), refuse held commands and
    /// drive work, save the layout, stop the Bus. Without a Bus the window
    /// closes at once.
    fn quit(&mut self) {
        if self.exiting {
            return;
        }
        self.exiting = true;
        self.app.cancel_dialogs();
        let derived = self.app.core.flush_config();
        self.app.absorb(derived);
        for cmd in std::mem::take(&mut self.held) {
            self.refuse_closing(cmd.id);
        }
        for answer in drive::finish(&self.ctx) {
            self.respond(answer.id, answer.rc, answer.body.to_string());
        }
        match &self.bus {
            Some(bus) => bus.handle.quit(),
            None => self.ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    /// The session theme again: settingsd's look when this build can draw
    /// it, else the theme file's.
    fn set_base(&mut self) {
        self.base = self
            .look
            .as_ref()
            .map(|look| {
                toolkit::session_look::follow_look(look, Some(APP), self.custom_source, &self.file_theme)
            })
            .unwrap_or_else(|| self.file_theme.clone())
            .with_ui(self.common_ui.clone());
        self.installed = None;
        self.ctx.request_repaint();
    }

    /// Apply the profile's bindings for dopus over the default chords, and
    /// report the effective chords in `dopus.actions`.
    pub fn set_keymap(&mut self, keymap: toolkit::keymap::Overrides) {
        if keymap == self.keymap {
            return;
        }
        for note in self.commands.rebind(&keymap) {
            eprintln!("dopus: keymap: {note}");
        }
        self.keymap = keymap;
        let commands = &self.commands;
        self.app.actions = commands::action_table(|action| {
            commands
                .shortcuts_of(action)
                .iter()
                .map(toolkit::keymap::chord_text)
                .collect()
        });
    }

    /// Show the session look `look` (settingsd's), or the theme file's with
    /// `None`; the window's own choice stays on top.
    pub fn set_look(&mut self, look: Option<settings::view::Look>) {
        self.follow_look(look, false);
    }

    fn follow_look(&mut self, look: Option<settings::view::Look>, custom_source: bool) {
        self.custom_source = custom_source;
        self.look = look;
        self.set_base();
    }

    fn read_look(&mut self) {
        if self.exiting {
            return;
        }
        if let Some(read) = self.follower.read() {
            self.issue_look(read);
        }
    }

    fn issue_look(&mut self, read: settings::follow::Read) {
        self.look_retry += 1;
        if let Some(bus) = &self.bus {
            bus.handle.read_look(read);
        }
    }

    /// A look read answered: a new look is installed at once; without
    /// settingsd the theme file stands; a transient fault is retried.
    pub fn looked(&mut self, ticket: u64, reply: Option<(u8, String, Option<String>)>) {
        if self.exiting || self.follower.pending() != Some(ticket) {
            return;
        }
        let answer = reply
            .as_ref()
            .map(|(rc, body, error)| (*rc, body.as_str(), error.as_deref()));
        let (followed, next) = self.follower.answered(ticket, answer);
        match followed {
            Some(settings::follow::Followed::Look {
                look,
                custom_source,
                keymaps,
                ..
            }) => {
                toolkit::debug!(app = "dopus", "settings applied");
                self.common_ui = toolkit::session_look::ui(&look, Some(APP));
                self.follow_look(Some(*look), custom_source);
                self.set_keymap(keymaps.get(APP).cloned().unwrap_or_default());
            }
            // settingsd gone: the look falls back to the theme file, but the
            // person's bindings stay in force until a profile changes them.
            Some(settings::follow::Followed::Missing) if self.look.is_some() => {
                self.set_look(None);
            }
            _ => {}
        }
        match next {
            Some(read) => self.issue_look(read),
            None => {
                if let Some(delay) = self.follower.retry_delay() {
                    self.look_retry += 1;
                    if let Some(bus) = &self.bus {
                        bus.handle.retry_look(delay, self.look_retry);
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
            self.installed = Some(self.app.theme);
            self.ctx.request_repaint();
        }
    }

    /// One frame's logic; it runs even while the window is hidden.
    pub fn logic(&mut self, ctx: &egui::Context) {
        if !self.deferred.is_empty() {
            let held = std::mem::take(&mut self.deferred);
            ctx.input_mut(|i| {
                i.events.splice(0..0, held);
            });
        }
        if let Some(bus) = &self.bus {
            drive::publish_titlebar(ctx, |topic, bodies, lost| {
                bus.handle.publish(topic, bodies, lost);
            });
        }
        for answer in drive::logic(ctx) {
            self.respond(answer.id, answer.rc, answer.body.to_string());
        }
        self.pump_core();
        self.app.sync_preview(ctx);
        self.pump_bus();
        self.perform();
        self.follow_theme();
        if ctx.input(|i| i.viewport().close_requested()) && !self.exiting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.app.quit();
            self.perform();
        }
        // Law 1: a frame at the core's next deadline, with no input.
        if let Some(at) = self.app.core.next_deadline() {
            let wait = at.saturating_duration_since(Instant::now());
            ctx.request_repaint_after(wait.max(Duration::from_millis(1)));
        }
    }

    /// Keep a frame's clicks and shortcuts in the order they came.
    /// Shortcuts run before the view applies the frame's clicks, so a
    /// shortcut after a click (select B, then Delete) waits, with everything
    /// after it, for the next frame, when the click has been applied: it
    /// acts on B, never on the row selected before.
    fn split_input(&mut self, ctx: &egui::Context) {
        let (commands, app) = (&self.commands, &self.app);
        // The presses `Registry::shortcuts` may take; clipboard events stand
        // for Ctrl+X, Ctrl+C and Ctrl+V.
        let is_shortcut = |e: &egui::Event| {
            commands.takes(e, app)
                || matches!(
                    e,
                    egui::Event::Cut
                        | egui::Event::Copy
                        | egui::Event::Paste(_)
                        | egui::Event::PasteImage(_)
                )
        };
        let rest = ctx.input_mut(|i| {
            let mut clicked = false;
            let at = i.events.iter().position(|e| {
                if is_shortcut(e) {
                    return clicked;
                }
                clicked |= matches!(e, egui::Event::PointerButton { pressed: true, .. });
                false
            });
            at.map(|n| i.events.split_off(n))
        });
        if let Some(rest) = rest {
            self.deferred = rest;
            ctx.request_repaint();
        }
    }

    /// One frame's drawing and interaction.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        // Shortcuts are off while a text field (the location bar, a prompt)
        // has the keyboard, a menu is open, or a dialog is up.
        let typing =
            ui.ctx().memory(|m| m.focused().is_some()) && ui.ctx().egui_wants_keyboard_input();
        if !typing && !toolkit::menu::is_open(ui.ctx()) && self.app.dialog.is_none() {
            self.split_input(ui.ctx());
            let fired = self.commands.shortcuts(ui.ctx(), &self.app);
            let events = fired
                .into_iter()
                .map(|id| crate::view::UiEvent::Command(id, toolkit::action::Origin::Shortcut))
                .collect();
            apply(&mut self.app, &self.commands, events);
            self.perform();
        }
        let events = view(ui, &self.app, &self.commands, &self.strings);
        apply(&mut self.app, &self.commands, events);
        let derived = self.app.core.tick(Instant::now());
        self.app.absorb(derived);
        self.perform();
        self.release();
        self.publish_executed();
        if self.installed != Some(self.app.theme) {
            ui.ctx().request_repaint();
        }
        let title = crate::view::title(&self.app);
        if title != self.title {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// Record the actions `dopus.execute` ran (their effects done) as Bus
    /// runs of their commands, and publish them.
    fn note_bus_runs(&mut self) {
        for id in std::mem::take(&mut self.app.ran) {
            if self.commands.get(&id).is_some() {
                self.commands.note(&id, toolkit::action::Origin::Bus);
            }
        }
        self.publish_executed();
    }

    /// Publish the commands run since the last call on
    /// `<service>.action.executed`. Without a Bus they are dropped.
    fn publish_executed(&self) {
        let (runs, lost) = self.commands.take_executed();
        let Some(bus) = &self.bus else { return };
        if runs.is_empty() && lost == 0 {
            return;
        }
        let topic = toolkit::executed::topic(bus.handle.service());
        let bodies = runs.iter().map(toolkit::executed::Executed::body).collect();
        bus.handle.publish(&topic, bodies, lost);
    }
}

impl eframe::App for Shell {
    fn on_exit(&mut self) {
        toolkit::macros::close_app(&self.ctx, "dopus");
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        Shell::logic(self, ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        Shell::ui(self, ui);
    }
}
