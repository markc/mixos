// SPDX-License-Identifier: MIT OR Apache-2.0
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
//! toolkit accepted before it is still unanswered, so a `dopus.action`
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
use crate::verbs::{self, ActionReply, AppearanceState, Command, ServerMeta, ThemeAction};
use crate::view::{apply, view};

/// The app prefix of dopus's verbs, and of its drive verbs.
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
    look: Option<settings::Appearance>,
    follower: settings::follow::Follower,
    look_topic: String,
    look_retry: u64,
    installed: Option<toolkit::theme_menu::Choice>,
    bus: Option<Bus>,
    core_events: mpsc::Receiver<CoreEvent>,
    ctx: egui::Context,
    service: String,
    config_path: Option<String>,
    /// Bus commands waiting behind drive input accepted before them.
    held: VecDeque<Command>,
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
        let commands = crate::commands::registry(&app.keymap);
        let shell = Self {
            app,
            commands,
            strings: crate::strings(),
            base: theme.clone(),
            file_theme: theme.clone(),
            theme,
            look: None,
            follower: settings::follow::Follower::new(&hostname()),
            look_topic: settings::summary_topic(settings::follow::PROFILE),
            look_retry: 0,
            installed: None,
            bus,
            core_events: rx,
            ctx,
            service,
            config_path,
            held: VecDeque::new(),
            exiting: false,
            title: String::new(),
        };
        if let Some(bus) = &shell.bus {
            // Hints arrive once the subscription lands; the read then
            // (`Delivery::Subscribed`) catches any change missed before.
            for topic in [shell.look_topic.clone(), THEME_TOPIC.into(), PROPS_TOPIC.into()] {
                bus.handle.subscribe(topic);
            }
        }
        for note in shell.app.keymap.notes.clone() {
            eprintln!("dopus: keymap: {note}");
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
            extra_verbs: drive::VERBS.iter().map(|v| format!("{APP}.{v}")).collect(),
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
        while let Some(delivery) = self
            .bus
            .as_mut()
            .and_then(|b| b.deliveries.try_recv().ok())
        {
            match delivery {
                Delivery::Command(cmd) => {
                    if self.exiting {
                        self.refuse_closing(cmd.id);
                    } else {
                        self.held.push_back(cmd);
                        self.release();
                    }
                }
                Delivery::Topic { topic, body } => {
                    if topic == self.look_topic {
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
                    if topic == self.look_topic {
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

    /// Run held Bus commands in order. Drive verbs that only join the drive
    /// queue go straight on; anything else waits for drive input before it.
    fn release(&mut self) {
        while let Some(cmd) = self.held.front() {
            let drive_verb = cmd
                .verb
                .strip_prefix("dopus.")
                .filter(|v| drive::VERBS.contains(v))
                .map(str::to_owned);
            let joins_queue = drive_verb.as_deref().is_some_and(drive::queues);
            if self.exiting || (drive::pending(&self.ctx) && !joins_queue) {
                return;
            }
            let cmd = self.held.pop_front().expect("a held command");
            match drive_verb {
                Some(verb) => {
                    let args: Value = serde_json::from_str(&cmd.body).unwrap_or(Value::Null);
                    if let Some(answer) = drive::request(&self.ctx, cmd.id, &verb, &args) {
                        self.respond(answer.id, answer.rc, answer.body.to_string());
                    }
                }
                None => {
                    let meta = self.meta();
                    self.app.serve(&cmd, &meta);
                    self.perform();
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
                let body = serde_json::to_string(&verbs::Refusal {
                    error_code: verbs::code::INVALID_ARGUMENT.into(),
                    message,
                    reason: None,
                })
                .unwrap_or_default();
                self.respond(id, 10, body);
            }
            None => {
                let body = serde_json::to_string(&verbs::ThemeSetReply {
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
            .and_then(toolkit::session_look::theme)
            .unwrap_or_else(|| self.file_theme.clone());
        self.installed = None;
        self.ctx.request_repaint();
    }

    /// Show the session look `look` (settingsd's), or the theme file's with
    /// `None`; the window's own choice stays on top.
    pub fn set_look(&mut self, look: Option<settings::Appearance>) {
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
                appearance,
                custom_source,
                ..
            }) => self.set_look((!custom_source).then_some(appearance)),
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
            self.theme = self.base.with_choice(&self.app.theme);
            toolkit::install(&self.ctx, &self.theme);
            self.installed = Some(self.app.theme);
            self.ctx.request_repaint();
        }
    }

    /// One frame's logic; it runs even while the window is hidden.
    pub fn logic(&mut self, ctx: &egui::Context) {
        for answer in drive::logic(ctx) {
            self.respond(answer.id, answer.rc, answer.body.to_string());
        }
        self.pump_core();
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

    /// One frame's drawing and interaction.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        // Shortcuts are off while a text field (the location bar, a prompt)
        // has the keyboard, a menu is open, or a dialog is up.
        let typing = ui.ctx().memory(|m| m.focused().is_some())
            && ui.ctx().egui_wants_keyboard_input();
        if !typing && !toolkit::menu::is_open(ui.ctx()) && self.app.dialog.is_none() {
            let fired = self.commands.shortcuts(ui.ctx(), &self.app);
            let events = fired
                .into_iter()
                .map(crate::view::UiEvent::Command)
                .collect();
            apply(&mut self.app, &self.commands, events);
            self.perform();
        }
        let stroke = toolkit::icons::stroke_width(&self.theme);
        let events = view(ui, &self.app, &self.commands, &self.strings, stroke);
        apply(&mut self.app, &self.commands, events);
        let derived = self.app.core.tick(Instant::now());
        self.app.absorb(derived);
        self.perform();
        self.release();
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
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        Shell::logic(self, ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        Shell::ui(self, ui);
    }
}
