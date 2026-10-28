// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The running window: hands Bus deliveries to the controller, performs the
//! effects the window owns (clipboard, the session file, Mix relex on worker
//! threads, quitting) and hands the rest to the transport. Nothing polls:
//! the Bus thread and the relex workers request a repaint when they finish,
//! and egui runs a frame.
//!
//! Ordering:
//! - Deliveries are applied in arrival order, each with its effects
//!   performed before the next. A Bus command waits while drive input the
//!   toolkit accepted before it is still unanswered (as in every MixOS app),
//!   so a `ced.execute` never overtakes a `ced.ui.click` sent first.
//! - Input keeps its order: the registry takes a frame's shortcuts before
//!   the editor sees the frame, so a shortcut that follows editor input
//!   waits for the next frame (`split_input`). Shortcuts are off while
//!   another text field (the find bar, a dialog) has the keyboard.
//! - Quitting finishes the effect batch it came in, so its own answer goes
//!   out, then answers every open dialog, held command and drive request
//!   before the Bus stops.
//!
//! The session theme is the look settingsd holds for the desktop profile,
//! followed through a [`settings::follow::Follower`]: a read at start, on
//! each hint on the profile's summary topic, on a reconnection and when
//! services register. Without settingsd, or with a look this build cannot
//! draw, the theme file's session theme stands (and follows
//! `theme.changed`). The window's own choice lays over either.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use documents::controller::{BusCommand, Effect};
use documents::session::SessionWriter;
use editor_model::highlight::ResultTag;
use editor_model::types::{Incoming, Intent, TabId};
use serde_json::{Value, json};
use toolkit::{Registry, Strings, Theme, drive};

use crate::app::App;
use crate::bus::{BusHandle, Delivery};
use crate::view::{UiEvent, view};

/// The app prefix of ced's commands, and of its drive commands.
pub const APP: &str = "ced";

/// How long a menu or Bus paste waits for the platform's clipboard, in
/// seconds.
const PASTE_WAIT: f64 = 2.0;

/// Where the controller's Bus effects go and its deliveries come from.
pub trait Transport {
    /// Perform a Bus effect (`Send`, `Respond`, `Timer`, `Subscribe`).
    fn perform(&mut self, effect: &Effect);
    /// The next delivery, if one has arrived.
    fn poll(&mut self) -> Option<Delivery>;
    fn connected(&self) -> bool;
    /// Perform a session-look read; [`Delivery::Look`] answers it.
    fn read_look(&mut self, read: settings::follow::Read);
    /// Deliver [`Delivery::LookRetry`] with `generation` after `delay`.
    fn retry_look(&mut self, delay: Duration, generation: u64);
    /// Stop: answer what is queued, flush `session`, then deliver
    /// [`Delivery::Stopped`].
    fn shutdown(&mut self, session: Option<SessionWriter>);
    /// Queue `bodies` for publishing on `topic`, oldest first; `lost`
    /// events were lost before them. A transport with no Bus drops them.
    fn publish(&mut self, topic: &str, bodies: Vec<Value>, lost: u64) {
        let _ = (topic, bodies, lost);
    }
    /// The Bus service name ced registered.
    fn service(&self) -> &str {
        APP
    }
}

/// The Bus thread as a transport.
pub struct Bus {
    pub handle: BusHandle,
    pub deliveries: futures::channel::mpsc::UnboundedReceiver<Delivery>,
}

impl Transport for Bus {
    fn perform(&mut self, effect: &Effect) {
        self.handle.perform(effect);
    }
    fn poll(&mut self) -> Option<Delivery> {
        self.deliveries.try_recv().ok()
    }
    fn connected(&self) -> bool {
        self.handle.connected()
    }
    fn read_look(&mut self, read: settings::follow::Read) {
        self.handle.read_look(read);
    }
    fn retry_look(&mut self, delay: Duration, generation: u64) {
        self.handle.retry_look(delay, generation);
    }
    fn shutdown(&mut self, session: Option<SessionWriter>) {
        self.handle.shutdown(session);
    }
    fn publish(&mut self, topic: &str, bodies: Vec<Value>, lost: u64) {
        self.handle.publish(topic, bodies, lost);
    }
    fn service(&self) -> &str {
        self.handle.service()
    }
}

/// This host's name: the settingsd instance to try first (settingsd runs
/// as `--instance %H`; the follower adopts the right one from its answer).
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}

/// Whether a `noded.props.changed` body may mean a service (settingsd)
/// came or went: a `services.registered` change, or one that cannot be read.
fn services_changed(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_none_or(|b| b["path"] == "services.registered")
}

type Relexed = (
    TabId,
    ResultTag,
    Vec<(std::ops::Range<usize>, editor_model::highlight::TokenClass)>,
);

pub struct Shell {
    pub app: App,
    commands: Registry<App>,
    strings: Strings,
    /// The session's theme (settingsd's look, else the theme file's), and
    /// the window's choice over it as installed.
    base: Theme,
    theme: Theme,
    /// The theme file's session theme: the fallback while settingsd is
    /// absent, or holds a look this build cannot draw.
    file_theme: Theme,
    /// The session look settingsd holds, once read.
    look: Option<settings::Appearance>,
    common_ui: settings::model::CommonUi,
    /// The profile's bindings for ced, as last applied.
    keymap: toolkit::keymap::Overrides,
    follower: settings::follow::Follower,
    /// The follower's hint topic (the profile's summary topic; a
    /// retarget changes only the instance, never the profile).
    look_topic: String,
    /// The latest look retry; an older one firing is void.
    look_retry: u64,
    installed: Option<toolkit::theme_menu::Choice>,
    palette: editor::Palette,
    transport: Box<dyn Transport>,
    ctx: egui::Context,
    relexed_tx: mpsc::Sender<Relexed>,
    relexed: mpsc::Receiver<Relexed>,
    session_path: Option<PathBuf>,
    writer: Option<SessionWriter>,
    /// Bus commands waiting behind drive input accepted before them.
    held: VecDeque<BusCommand>,
    /// Menu or Bus pastes waiting for the clipboard, in order.
    pastes: VecDeque<Intent>,
    /// The deadline of the clipboard request out for the first of them.
    asked: Option<f64>,
    /// Input held for the next frame (see `split_input`).
    deferred: Vec<egui::Event>,
    exiting: bool,
}

impl Shell {
    pub fn new(
        ctx: egui::Context,
        theme: Theme,
        mut app: App,
        transport: Box<dyn Transport>,
        session_path: Option<PathBuf>,
    ) -> Self {
        drive::install(&ctx, APP);
        let (relexed_tx, relexed) = mpsc::channel();
        let fx = app.ctl.start();
        app.absorb(fx);
        let palette = editor::Palette::from_theme(&theme);
        let mut shell = Self {
            app,
            commands: crate::actions::registry(),
            strings: crate::strings(),
            base: theme.clone(),
            file_theme: theme.clone(),
            theme,
            look: None,
            common_ui: settings::model::CommonUi::default(),
            keymap: toolkit::keymap::Overrides::new(),
            follower: settings::follow::Follower::new(&hostname()),
            look_topic: settings::summary_topic(settings::follow::PROFILE),
            look_retry: 0,
            installed: None,
            palette,
            transport,
            ctx,
            relexed_tx,
            relexed,
            writer: session_path.as_ref().map(|_| SessionWriter::spawn()),
            session_path,
            held: VecDeque::new(),
            pastes: VecDeque::new(),
            asked: None,
            deferred: Vec::new(),
            exiting: false,
        };
        shell.perform();
        // Hints arrive only once the subscription lands; the read then
        // (`Delivery::Subscribed`) catches any change an earlier read (a
        // reconnection, a registration) could have missed.
        let topic = shell.look_topic.clone();
        shell.transport.perform(&Effect::Subscribe { topic });
        shell
    }

    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }

    /// Apply every delivery that has arrived, performing effects after each.
    fn pump(&mut self) {
        while let Some(delivery) = self.transport.poll() {
            match delivery {
                Delivery::MailSaved { tab, path, text, send, command, result } => {
                    let fx = self.app.ctl.on_mail_saved(tab, path, text, send, command, result);
                    self.app.absorb(fx);
                }
                Delivery::MailSent { tab, command, result } => {
                    let fx = self.app.ctl.on_mail_sent(tab, command, result);
                    self.app.absorb(fx);
                }
                Delivery::NotificationFailed { text } => self.app.notification_failed(text),
                Delivery::Incoming(Incoming::Topic { topic, .. }) if topic == self.look_topic => {
                    // A hint, never trusted: the look comes from the read.
                    // The window's own topic: the controller never sees it.
                    self.read_look();
                }
                Delivery::Incoming(incoming) => {
                    match &incoming {
                        Incoming::Topic { topic, .. } if topic == "theme.changed" => {
                            // The theme file changed: it is the session
                            // theme while settingsd's look is not drawn.
                            self.file_theme = Theme::load();
                            self.set_base();
                        }
                        // settingsd may have come (or come back).
                        Incoming::Topic { topic, body }
                            if topic == "noded.props.changed" && services_changed(body) =>
                        {
                            self.read_look();
                        }
                        Incoming::Connection { up: true } => self.read_look(),
                        _ => {}
                    }
                    let fx = self.app.ctl.on_incoming(incoming);
                    self.app.absorb(fx);
                }
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
                Delivery::Command(cmd) => {
                    if self.exiting {
                        self.refuse_closing(cmd.id);
                    } else {
                        self.held.push_back(cmd);
                        self.release();
                    }
                }
                Delivery::Stopped { faults } => {
                    for fault in faults {
                        eprintln!("ced: {fault}");
                    }
                    self.exiting = true;
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            self.perform();
        }
        while let Ok((tab, tag, spans)) = self.relexed.try_recv() {
            self.app.ctl.on_relex(tab, tag, spans);
        }
        self.release();
    }

    /// Run held Bus commands in order. Drive commands that only join the drive
    /// queue go straight on; anything else waits for drive input before it.
    fn release(&mut self) {
        while let Some(cmd) = self.held.front() {
            let drive_command = cmd
                .command
                .strip_prefix("ced.")
                .filter(|v| drive::COMMANDS.contains(v))
                .map(str::to_owned);
            let joins_queue = drive_command.as_deref().is_some_and(drive::queues);
            if self.exiting || (drive::pending(&self.ctx) && !joins_queue) {
                return;
            }
            let cmd = self.held.pop_front().expect("a held command");
            if cmd.command == "HELP" {
                self.respond(drive::Answer {
                    id: cmd.id,
                    rc: 0,
                    body: json!(crate::bus::help(true)),
                });
                continue;
            }
            match drive_command {
                Some(command_name) => {
                    let args: Value = serde_json::from_str(&cmd.body).unwrap_or(Value::Null);
                    if let Some(answer) = drive::request(&self.ctx, cmd.id, &command_name, &args) {
                        self.respond(answer);
                    }
                }
                None => {
                    self.app.sync_view_hints();
                    let fx = self.app.ctl.on_bus_command(cmd);
                    self.app.absorb(fx);
                    crate::view::record_noted(&mut self.app, &self.commands);
                    self.perform();
                }
            }
        }
    }

    fn respond(&mut self, answer: drive::Answer) {
        self.transport.perform(&Effect::Respond {
            id: answer.id,
            rc: answer.rc,
            body: answer.body.to_string(),
        });
    }

    fn refuse_closing(&mut self, id: u64) {
        let body = json!({"error_code":"BUSY","message":"ced is closing"}).to_string();
        self.transport
            .perform(&Effect::Respond { id, rc: 10, body });
    }

    /// Perform the effects the controller left for the shell. A quit waits
    /// until the rest of its batch (its own answer, a session save) is done.
    fn perform(&mut self) {
        let mut quit = false;
        while !self.app.effects.is_empty() {
            for effect in std::mem::take(&mut self.app.effects) {
                match effect {
                    Effect::ClipboardWrite { text, primary } => {
                        // The primary selection is the compositor's; egui
                        // writes the clipboard only.
                        if !primary {
                            self.ctx.copy_text(text);
                        }
                    }
                    Effect::ClipboardRead { primary, intent } => {
                        if primary {
                            // Not ced's to read: answered as empty.
                            let fx = self.app.ctl.on_paste(intent, None);
                            self.app.absorb(fx);
                        } else {
                            // The platform answers with a paste event, which
                            // `take_pastes` hands back with this intent.
                            self.pastes.push_back(intent);
                            self.ask_paste();
                        }
                    }
                    Effect::SaveSession => self.save_session(),
                    Effect::Relex { tab, tag, source } => self.relex(tab, tag, source),
                    Effect::Quit => quit = true,
                    other => self.transport.perform(&other),
                }
            }
        }
        if quit {
            self.quit();
        }
    }

    /// Paste events answering a menu or Bus paste go to that paste's intent
    /// (its tab and caller), not to whatever has the keyboard.
    ///
    /// The platform answers a paste request on the next frame, or not at all
    /// when the clipboard holds no text, and its answer carries no tag. So
    /// one request is out at a time, and it is answered as empty when its
    /// deadline passes. Paste events beyond it are the person's own and stay
    /// for the focused widget.
    fn take_pastes(&mut self, ctx: &egui::Context) {
        let Some(until) = self.asked else {
            return;
        };
        let text = ctx.input_mut(|i| {
            let at = i
                .events
                .iter()
                .position(|e| matches!(e, egui::Event::Paste(_)))?;
            match i.events.remove(at) {
                egui::Event::Paste(text) => Some(text),
                _ => None,
            }
        });
        let now = self.app.now;
        if text.is_some() || until <= now {
            self.asked = None;
            if let Some(intent) = self.pastes.pop_front() {
                let fx = self.app.ctl.on_paste(intent, text);
                self.app.absorb(fx);
            }
            self.perform();
            self.ask_paste();
        } else {
            ctx.request_repaint_after_secs((until - now) as f32);
        }
    }

    /// Ask the platform for the clipboard for the first waiting paste,
    /// unless a request is already out.
    fn ask_paste(&mut self) {
        if self.asked.is_none() && !self.pastes.is_empty() {
            self.asked = Some(self.app.now + PASTE_WAIT);
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        }
    }

    fn save_session(&self) {
        if let (Some(writer), Some(path)) = (&self.writer, &self.session_path)
            && let Err(error) = writer.queue(path.clone(), self.app.ctl.session())
        {
            eprintln!("ced: session not saved: {error}");
        }
    }

    /// Lex a Mix buffer on a worker thread; its spans come back through a
    /// channel that wakes the window.
    fn relex(&self, tab: TabId, tag: ResultTag, source: std::sync::Arc<str>) {
        let (tx, ctx) = (self.relexed_tx.clone(), self.ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("ced-relex".into())
            .spawn(move || {
                let spans = editor_model::highlight::run_mix(&tag.language, &source);
                let _ = tx.send((tab, tag, spans));
                ctx.request_repaint();
            });
        if let Err(error) = spawned {
            eprintln!("ced: relex thread: {error}");
        }
    }

    /// Detach and exit: answer open dialogs, held commands and drive work,
    /// save the session, stop the Bus. The window closes when the Bus thread
    /// has stopped.
    fn quit(&mut self) {
        if self.exiting {
            return;
        }
        self.exiting = true;
        self.app.cancel_dialogs();
        for effect in std::mem::take(&mut self.app.effects) {
            if crate::bus::is_bus(&effect) && !matches!(effect, Effect::Quit) {
                self.transport.perform(&effect);
            }
        }
        for cmd in std::mem::take(&mut self.held) {
            self.refuse_closing(cmd.id);
        }
        for answer in drive::finish(&self.ctx) {
            self.respond(answer);
        }
        self.save_session();
        self.transport.shutdown(self.writer.take());
    }

    /// The session theme again: settingsd's look when this build can draw
    /// it, else the theme file's. The next frame installs it under the
    /// window's choice.
    fn set_base(&mut self) {
        self.base = self
            .look
            .as_ref()
            .map(|look| toolkit::session_look::theme_or(look, &self.file_theme))
            .unwrap_or_else(|| self.file_theme.clone())
            .with_ui(self.common_ui.clone());
        self.installed = None;
        self.ctx.request_repaint();
    }

    /// Apply the profile's bindings for ced over the default chords, and
    /// report the keys in force in `ced.actions`.
    pub fn set_keymap(&mut self, keymap: toolkit::keymap::Overrides) {
        if keymap == self.keymap {
            return;
        }
        for note in self.commands.rebind(&keymap) {
            eprintln!("ced: keymap: {note}");
        }
        self.keymap = keymap;
        let keys = crate::actions::keys_in_force(&self.commands);
        self.app.ctl.set_keys(keys);
    }

    /// Show the session look `look` (settingsd's), or the theme file's with
    /// `None`; the window's own choice stays on top.
    pub fn set_look(&mut self, look: Option<settings::Appearance>) {
        self.look = look;
        self.set_base();
    }

    /// Read the session look (start, a settingsd hint, a reconnection). A
    /// read going out voids any armed retry; closing reads nothing.
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
        self.transport.read_look(read);
    }

    /// Ask for the session look again now (a hint, as from settingsd).
    pub fn hint_look(&mut self) {
        self.read_look();
    }

    /// The session look follower (tests read its tickets and retry state).
    pub fn follower(&self) -> &settings::follow::Follower {
        &self.follower
    }

    /// The look retry generation now; a retry of any other one is void.
    pub fn look_generation(&self) -> u64 {
        self.look_retry
    }

    /// A look read answered: a new look is installed at once (under the
    /// window's own choice); without settingsd the theme file stands; a
    /// transient fault is retried with backoff, the last look standing.
    pub fn looked(&mut self, ticket: u64, reply: Option<(u8, String, Option<String>)>) {
        // Closing: nothing more to draw. A superseded read's answer changes
        // nothing, retries included.
        if self.exiting || self.follower.pending() != Some(ticket) {
            return;
        }
        let answer = reply
            .as_ref()
            .map(|(rc, body, error)| (*rc, body.as_str(), error.as_deref()));
        let (followed, next) = self.follower.answered(ticket, answer);
        match followed {
            // A profile naming its own design source keeps the theme file,
            // as `session_look` does for one it cannot draw.
            Some(settings::follow::Followed::Look {
                appearance,
                ui,
                custom_source,
                keymaps,
                ced,
                ..
            }) => {
                toolkit::debug!(app = "ced", "settings applied");
                self.common_ui = ui;
                self.set_look((!custom_source).then_some(*appearance));
                self.set_keymap(keymaps.get(APP).cloned().unwrap_or_default());
                self.app.set_editor_style(ced.editor.clone());
                self.palette = self.editor_palette();
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
            Some(read) => self.issue_look(read),
            None => {
                if let Some(delay) = self.follower.retry_delay() {
                    self.look_retry += 1;
                    self.transport.retry_look(delay, self.look_retry);
                }
            }
        }
    }

    /// The editor's colours: the light theme when the editor's saved style
    /// says light, else the window's theme.
    fn editor_palette(&self) -> editor::Palette {
        if self.app.editor_style.light {
            editor::Palette::from_theme(toolkit::content::light_theme())
        } else {
            editor::Palette::from_theme(&self.theme)
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
            self.palette = self.editor_palette();
            self.installed = Some(self.app.theme);
            self.ctx.request_repaint();
        }
    }

    /// One frame's logic; it runs even while the window is hidden.
    pub fn logic(&mut self, ctx: &egui::Context) {
        drive::set_service(
            ctx,
            self.transport.connected().then(|| self.transport.service()),
        );
        self.app.now = ctx.input(|i| i.time);
        if !self.deferred.is_empty() {
            // Input held back last frame comes before this frame's.
            let held = std::mem::take(&mut self.deferred);
            ctx.input_mut(|i| {
                i.events.splice(0..0, held);
            });
        }
        self.perform();
        drive::publish_titlebar(ctx, |topic, bodies, lost| {
            self.transport.publish(topic, bodies, lost);
        });
        for answer in drive::logic(ctx) {
            self.respond(answer);
        }
        self.take_pastes(ctx);
        self.pump();
        self.app.prune();
        self.perform();
        self.follow_theme();
        if ctx.input(|i| i.viewport().close_requested()) && !self.exiting {
            // Closing detaches every document; the window goes once the Bus
            // has answered and stopped.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.app.action(documents::actions::ActionId::FileExit);
            self.perform();
        }
        // Runs the Bus made while the window may be hidden (no ui pass).
        self.publish_executed();
        // The status line fades without input.
        if let Some((_, until)) = self.app.status
            && until > self.app.now
        {
            ctx.request_repaint_after_secs((until - self.app.now) as f32);
        }
    }

    /// Keep a frame's shortcuts and editor input in the order they came.
    /// Shortcuts run before the editor sees the frame, so a shortcut after
    /// editor input (`x` then Ctrl+A) waits, with everything after it, for
    /// the next frame, when the input before it has been applied.
    ///
    /// While the editor still holds input from an earlier frame (it waits
    /// for its own edits to apply before a Copy), every shortcut waits too.
    fn split_input(&mut self, ctx: &egui::Context) {
        if toolkit::menu::is_open(ctx) {
            // No shortcut fires while a menu is open.
            return;
        }
        let (commands, app) = (&self.commands, &self.app);
        // Exactly the presses `Registry::shortcuts` will take.
        let is_shortcut = |e: &egui::Event| commands.takes(e, app);
        let edits = |e: &egui::Event| {
            matches!(
                e,
                egui::Event::Text(_)
                    | egui::Event::Key { pressed: true, .. }
                    | egui::Event::Paste(_)
                    | egui::Event::PasteImage(_)
                    | egui::Event::Copy
                    | egui::Event::Cut
                    | egui::Event::Ime(_)
                    | egui::Event::PointerButton { pressed: true, .. }
            )
        };
        let rest = ctx.input_mut(|i| {
            let mut seen = app.editor_held;
            let at = i.events.iter().position(|e| {
                if is_shortcut(e) {
                    return seen;
                }
                seen |= edits(e);
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
        // Another text field has the keyboard: its keys are its own.
        let focused = ui.ctx().memory(|m| m.focused());
        let typing_elsewhere = focused.is_some() && focused != self.app.editor_id;
        if !typing_elsewhere {
            self.split_input(ui.ctx());
            let fired = self.commands.shortcuts(ui.ctx(), &self.app);
            let events = fired
                .into_iter()
                .map(|id| UiEvent::Command(id, toolkit::action::Origin::Shortcut))
                .collect();
            crate::view::apply(&mut self.app, &self.commands, events);
            self.perform();
        }
        let connected = self.transport.connected();
        let events = view(
            ui,
            &mut self.app,
            &self.commands,
            &self.strings,
            &self.palette,
            connected,
        );
        crate::view::apply(&mut self.app, &self.commands, events);
        toolkit::markdown::open_links(ui.ctx(), "ced");
        self.perform();
        self.release();
        self.publish_executed();
    }

    /// Publish the commands run since the last call on
    /// `<service>.action.executed`.
    fn publish_executed(&mut self) {
        crate::view::record_noted(&mut self.app, &self.commands);
        let (runs, lost) = self.commands.take_executed();
        if runs.is_empty() && lost == 0 {
            return;
        }
        let topic = toolkit::executed::topic(self.transport.service());
        let bodies = runs.iter().map(toolkit::executed::Executed::body).collect();
        self.transport.publish(&topic, bodies, lost);
    }
}

impl eframe::App for Shell {
    fn on_exit(&mut self) {
        toolkit::macros::close_app(&self.ctx, "ced");
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        Shell::logic(self, ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        Shell::ui(self, ui);
    }
}

/// An incoming reply, as a transport delivers it (for test transports).
pub fn reply(req: u64, rc: u8, body: String) -> Delivery {
    Delivery::Incoming(Incoming::Reply { req, rc, body })
}
