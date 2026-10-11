// SPDX-License-Identifier: MIT OR Apache-2.0
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
//!   so a `ced.action` never overtakes a `ced.ui.click` sent first.
//! - Input keeps its order: the registry takes a frame's shortcuts before
//!   the editor sees the frame, so a shortcut that follows editor input
//!   waits for the next frame (`split_input`). Shortcuts are off while
//!   another text field (the find bar, a dialog) has the keyboard.
//! - Quitting finishes the effect batch it came in, so its own answer goes
//!   out, then answers every open dialog, held command and drive request
//!   before the Bus stops.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;

use documents::controller::{BusCommand, Effect};
use documents::session::SessionWriter;
use editor_model::highlight::ResultTag;
use editor_model::types::{Incoming, Intent, TabId};
use serde_json::{Value, json};
use toolkit::{Registry, Strings, Theme, drive};

use crate::app::App;
use crate::bus::{BusHandle, Delivery};
use crate::view::{UiEvent, view};

/// The app prefix of ced's verbs, and of its drive verbs.
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
    /// Stop: answer what is queued, flush `session`, then deliver
    /// [`Delivery::Stopped`].
    fn shutdown(&mut self, session: Option<SessionWriter>);
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
    fn shutdown(&mut self, session: Option<SessionWriter>) {
        self.handle.shutdown(session);
    }
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
    /// The session's theme, and the window's choice over it as installed.
    base: Theme,
    theme: Theme,
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
            commands: crate::commands::registry(),
            strings: crate::strings(),
            base: theme.clone(),
            theme,
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
                Delivery::Incoming(incoming) => {
                    if matches!(&incoming, Incoming::Topic { topic, .. } if topic == "theme.changed")
                    {
                        // The session theme changed: the window follows it,
                        // under its own choice.
                        self.base = Theme::load();
                        self.installed = None;
                    }
                    let fx = self.app.ctl.on_incoming(incoming);
                    self.app.absorb(fx);
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

    /// Run held Bus commands in order. Drive verbs that only join the drive
    /// queue go straight on; anything else waits for drive input before it.
    fn release(&mut self) {
        while let Some(cmd) = self.held.front() {
            let drive_verb = cmd
                .verb
                .strip_prefix("ced.")
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
                        self.respond(answer);
                    }
                }
                None => {
                    let fx = self.app.ctl.on_bus_command(cmd);
                    self.app.absorb(fx);
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

    fn follow_theme(&mut self) {
        if self.installed != Some(self.app.theme) {
            self.theme = self.base.with_choice(&self.app.theme);
            toolkit::install(&self.ctx, &self.theme);
            self.palette = editor::Palette::from_theme(&self.theme);
            self.installed = Some(self.app.theme);
            self.ctx.request_repaint();
        }
    }

    /// One frame's logic; it runs even while the window is hidden.
    pub fn logic(&mut self, ctx: &egui::Context) {
        self.app.now = ctx.input(|i| i.time);
        if !self.deferred.is_empty() {
            // Input held back last frame comes before this frame's.
            let held = std::mem::take(&mut self.deferred);
            ctx.input_mut(|i| {
                i.events.splice(0..0, held);
            });
        }
        self.perform();
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
            let events = fired.into_iter().map(UiEvent::Command).collect();
            crate::view::apply(&mut self.app, &self.commands, events);
            self.perform();
        }
        let stroke = toolkit::icons::stroke_width(&self.theme);
        let connected = self.transport.connected();
        let events = view(
            ui,
            &mut self.app,
            &self.commands,
            &self.strings,
            &self.palette,
            stroke,
            connected,
        );
        crate::view::apply(&mut self.app, &self.commands, events);
        self.perform();
        self.release();
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

/// An incoming reply, as a transport delivers it (for test transports).
pub fn reply(req: u64, rc: u8, body: String) -> Delivery {
    Delivery::Incoming(Incoming::Reply { req, rc, body })
}
