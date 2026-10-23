// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The running window: feeds the app its events and carries out its
//! effects. Bus work and captures run on a small tokio runtime and image
//! work on blocking threads; each completion comes back as an [`Event`]
//! with a repaint request, so the window wakes only when something happened
//! (no polling).
//!
//! **Ordering is the contract.** Each event is applied and its effects
//! performed before the next is read. Within a frame, edits are applied
//! before commands ([`crate::view::apply`]). Bus commands wait behind window
//! input the drive layer accepted before them, and every accepted command
//! is answered before the Bus stops.
//!
//! The session theme is the look settingsd holds for the desktop profile,
//! followed through a [`settings::follow::Follower`]; without it, the theme
//! file's. The window's own choice (View › Theme) lays over either.
use crate::app::SharedCommands;
use crate::app::{App, Effect, Pending};
use crate::capture::{Comp, CompBus, Mode, Target};
use crate::jobs::{self, Done};
use crate::view::{Drag, Preview, UiEvent, apply, view};
use crate::{APP_ID, SERVICE, label, strings};
use citizen::{CallError, Delivery, Handle, Reply};
use futures::StreamExt;
use image::RgbaImage;
use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::task::Poll;
use toolkit::command::Origin;
use toolkit::{Strings, Theme, drive};

/// What finished off the UI thread.
pub enum Event {
    Delivery(Delivery),
    /// A finished job (boxed: a capture carries a whole document).
    Done(Box<Done>),
    /// A preview render.
    Preview(Rendered),
    /// The render for copy job `.0`.
    Copied(u64, Result<RgbaImage, String>),
    /// A read of the session look (settingsd).
    Look(u64, Result<Reply, CallError>),
    /// A retry of the session look (its generation).
    LookRetry(u64),
}

/// A finished job as an event.
fn done(done: Done) -> Event {
    Event::Done(Box::new(done))
}

/// A preview render of one revision of one document.
pub struct Rendered {
    document: u64,
    revision: u64,
    /// The document's output size when it was rendered.
    size: [usize; 2],
    image: Result<RgbaImage, String>,
}

/// The largest preview texture side; larger shots are scaled to fit.
const TEXTURE_LIMIT: u32 = 8192;

/// What the command line asked for at start.
#[derive(Clone, Debug, Default)]
pub struct Start {
    /// An image to open.
    pub path: Option<PathBuf>,
    /// A capture to take at once (Print Screen's launch).
    pub capture: Option<(Mode, Option<Target>)>,
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
const LOOK_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

pub struct Shell {
    app: App,
    commands: SharedCommands,
    strings: Strings,
    /// The session theme, and the theme file's (the fallback).
    base: Theme,
    file_theme: Theme,
    theme: Theme,
    installed: Option<toolkit::theme_menu::Choice>,
    look: Option<settings::Appearance>,
    keymap: toolkit::keymap::Overrides,
    follower: settings::follow::Follower,
    look_retry: u64,
    bus: Handle,
    comp: Arc<dyn Comp>,
    comp_service: String,
    runtime: tokio::runtime::Runtime,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    ctx: egui::Context,
    exiting: bool,
    /// Bus commands waiting behind drive input accepted before them.
    held: VecDeque<(u64, String, String)>,
    inbox: Arc<Mutex<Deliveries>>,
    cancel: Option<tokio::sync::watch::Sender<bool>>,
    preview: Option<Preview>,
    /// The document and revision shown, and the revision being rendered.
    shown: Option<(u64, u64)>,
    rendering: Option<u64>,
    drag: Drag,
    title: String,
}

impl Shell {
    pub fn new(
        ctx: egui::Context,
        theme: Theme,
        bus: Handle,
        deliveries: Deliveries,
        comp: String,
        start: Start,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("screenshot-effects")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        drive::install(&ctx, SERVICE);
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
        let mut app = App::new(label, crate::ink::inks(), false);
        app.comp = comp.clone();
        app.refresh();
        if let Some(path) = start.path {
            app.request(Pending::Open(path));
        }
        if let Some((mode, window)) = start.capture {
            if window.is_some() {
                app.choose_window(window);
            }
            app.take(Some(mode));
        }
        let comp_bus: Arc<dyn Comp> = Arc::new(CompBus {
            handle: bus.clone(),
            service: comp.clone(),
        });
        let mut shell = Self {
            commands: app.commands.clone(),
            app,
            strings: strings(),
            base: theme.clone(),
            file_theme: theme.clone(),
            theme,
            installed: None,
            look: None,
            keymap: toolkit::keymap::Overrides::new(),
            follower: settings::follow::Follower::new(&hostname()),
            look_retry: 0,
            bus,
            comp: comp_bus,
            comp_service: comp,
            runtime,
            tx,
            rx,
            ctx,
            exiting: false,
            held: VecDeque::new(),
            inbox,
            cancel: None,
            preview: None,
            shown: None,
            rendering: None,
            drag: Drag::default(),
            title: String::new(),
        };
        // Raw captures from earlier sessions are bounded by count and age.
        shell.runtime.spawn_blocking(jobs::prune);
        shell.settle();
        shell.read_look();
        Ok(shell)
    }

    pub fn app(&self) -> &App {
        &self.app
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
            match event {
                Event::Delivery(Delivery::Command { id, verb, body }) => {
                    if self.exiting {
                        self.refuse_closing(id);
                    } else {
                        self.held.push_back((id, verb, body));
                        self.release();
                    }
                    continue;
                }
                Event::Delivery(Delivery::Theme) => {
                    self.file_theme = Theme::load();
                    self.set_base();
                }
                Event::Delivery(Delivery::Topic(topic)) => {
                    if topic == self.follower.topic() {
                        self.read_look();
                    }
                }
                Event::Delivery(Delivery::Connected) => {
                    self.app.connection(true);
                    self.read_look();
                }
                Event::Delivery(Delivery::Changed) => self.read_look(),
                Event::Delivery(Delivery::Disconnected) => self.app.connection(false),
                Event::Done(done) => {
                    if matches!(*done, Done::Captured(..)) {
                        self.cancel = None;
                    }
                    jobs::apply(&mut self.app, *done);
                }
                Event::Preview(rendered) => self.previewed(rendered),
                Event::Copied(ticket, result) => {
                    let result = result.map(|image| {
                        let size = [image.width() as usize, image.height() as usize];
                        self.ctx
                            .copy_image(egui::ColorImage::from_rgba_unmultiplied(size, &image));
                    });
                    self.app.copied(ticket, result);
                }
                Event::Look(ticket, result) => self.looked(ticket, result),
                Event::LookRetry(generation) => {
                    if generation == self.look_retry && !self.exiting {
                        self.read_look();
                    }
                }
            }
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
        while let Some((_, verb, _)) = self.held.front() {
            let joins_queue = verb.strip_prefix("screenshot.").is_some_and(drive::queues);
            if self.exiting || (drive::pending(&self.ctx) && !joins_queue) {
                return;
            }
            let (id, verb, body) = self.held.pop_front().expect("a held command");
            self.app.command(id, &verb, &body);
            self.settle();
        }
    }

    fn settle(&mut self) {
        loop {
            let effects = jobs::settle(&mut self.app, &self.commands.borrow(), &self.strings, true);
            if effects.is_empty() {
                break;
            }
            for effect in effects {
                self.perform(effect);
            }
        }
        self.publish_executed();
    }

    /// Publish the commands run since the last call on
    /// `<service>.command.executed`.
    fn publish_executed(&self) {
        let (runs, lost) = self.commands.borrow().take_executed();
        if runs.is_empty() && lost == 0 {
            return;
        }
        let topic = toolkit::executed::topic(self.bus.service());
        let bodies = runs.iter().map(toolkit::executed::Executed::body).collect();
        self.bus.publish(&topic, bodies, lost);
    }

    fn perform(&mut self, effect: Effect) {
        match effect {
            Effect::Reply { id, rc, body } => self.bus.reply(id, rc, body),
            Effect::Publish { suffix, body } => {
                let topic = format!("{}.{suffix}", self.bus.service());
                self.bus.publish(&topic, vec![body], 0);
            }
            Effect::Capture {
                ticket,
                request,
                hide,
                focus,
            } => {
                let (tx, rx) = tokio::sync::watch::channel(false);
                self.cancel = Some(tx);
                let comp = self.comp.clone();
                self.spawn(async move {
                    done(jobs::capture(comp, ticket, request, hide, focus, rx).await)
                });
            }
            Effect::Cancel => {
                if let Some(tx) = &self.cancel {
                    let _ = tx.send(true);
                }
            }
            Effect::Open { ticket, path } => {
                self.spawn(async move { done(jobs::open(ticket, path).await) });
            }
            Effect::Save {
                ticket,
                document,
                to,
            } => {
                let face = crate::face();
                self.spawn(async move { done(jobs::save(ticket, *document, to, face).await) });
            }
            Effect::Copy { ticket, document } => {
                let face = crate::face();
                self.spawn(
                    async move { Event::Copied(ticket, jobs::render(*document, face).await) },
                );
            }
            Effect::Show { ticket } => {
                let (bus, comp) = (self.bus.clone(), self.comp_service.clone());
                self.spawn(async move {
                    let result = citizen::show(bus, comp, APP_ID).await;
                    match ticket {
                        Some(ticket) => done(Done::Shown(ticket, result)),
                        None => done(Done::Raised(result)),
                    }
                });
            }
            Effect::Refresh => {
                let comp = self.comp.clone();
                self.spawn(async move { done(jobs::refresh(comp).await) });
            }
            Effect::Forget { path } => {
                self.runtime.spawn_blocking(move || jobs::forget(&path));
            }
            Effect::Drive { id, verb, args } => {
                if let Some(answer) = drive::request(&self.ctx, id, &verb, &args) {
                    self.bus.reply(answer.id, answer.rc, answer.body);
                }
            }
            Effect::Exit => self.exit(),
            // `jobs::settle` answers these.
            Effect::Commands { .. }
            | Effect::Execute { .. }
            | Effect::Describe { .. }
            | Effect::Recent { .. } => {}
        }
    }

    /// Close: every accepted command is answered before the Bus stops.
    fn exit(&mut self) {
        if self.exiting {
            return;
        }
        self.exiting = true;
        if let Some(tx) = &self.cancel {
            let _ = tx.send(true);
        }
        for answer in drive::finish(&self.ctx) {
            self.bus.reply(answer.id, answer.rc, answer.body);
        }
        let mut refused: Vec<u64> = self.held.drain(..).map(|(id, ..)| id).collect();
        {
            let mut inbox = locked(&self.inbox);
            inbox.close();
            while let Ok(delivery) = inbox.try_recv() {
                if let Delivery::Command { id, .. } = delivery {
                    refused.push(id);
                }
            }
            while let Ok(event) = self.rx.try_recv() {
                if let Event::Delivery(Delivery::Command { id, .. }) = event {
                    refused.push(id);
                }
            }
        }
        for id in refused {
            self.refuse_closing(id);
        }
        self.bus.quit();
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Keep the preview on the document's latest revision: one render at a
    /// time, the newest revision next. A replaced document drops the
    /// preview at once, and a render of an older document is discarded.
    fn follow_preview(&mut self) {
        let Some(document) = &self.app.document else {
            self.preview = None;
            self.shown = None;
            return;
        };
        let id = self.app.document_id;
        if self.preview.as_ref().is_some_and(|p| p.document != id) {
            self.preview = None;
        }
        if self.rendering.is_some() || self.shown == Some((id, self.app.revision)) {
            return;
        }
        let revision = self.app.revision;
        self.rendering = Some(revision);
        let (w, h) = document.output_dimensions();
        let size = [w as usize, h as usize];
        let document = document.raster_snapshot();
        let face = crate::face();
        self.spawn(async move {
            let rendered = jobs::render(document, face).await.map(|image| {
                if image.width() > TEXTURE_LIMIT || image.height() > TEXTURE_LIMIT {
                    let scale =
                        f64::from(TEXTURE_LIMIT) / f64::from(image.width().max(image.height()));
                    image::imageops::resize(
                        &image,
                        ((f64::from(image.width()) * scale) as u32).max(1),
                        ((f64::from(image.height()) * scale) as u32).max(1),
                        image::imageops::FilterType::Triangle,
                    )
                } else {
                    image
                }
            });
            Event::Preview(Rendered {
                document: id,
                revision,
                size,
                image: rendered,
            })
        });
    }

    fn previewed(&mut self, rendered: Rendered) {
        self.rendering = None;
        // Another document since: discard it; the next frame renders anew.
        if rendered.document != self.app.document_id || self.app.document.is_none() {
            self.ctx.request_repaint();
            return;
        }
        match rendered.image {
            Ok(image) => {
                let pixels = egui::ColorImage::from_rgba_unmultiplied(
                    [image.width() as usize, image.height() as usize],
                    &image,
                );
                let options = egui::TextureOptions::LINEAR;
                if let Some(preview) = &mut self.preview {
                    preview.texture.set(pixels, options);
                    preview.size = rendered.size;
                } else {
                    let texture = self.ctx.load_texture("screenshot-preview", pixels, options);
                    self.preview = Some(Preview {
                        texture,
                        size: rendered.size,
                        document: rendered.document,
                    });
                }
            }
            Err(error) => self.app.status = format!("{}: {error}", label("error")),
        }
        self.shown = Some((rendered.document, rendered.revision));
        self.ctx.request_repaint();
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

    /// Apply the profile's bindings for screenshot over the built chords.
    pub fn set_keymap(&mut self, keymap: toolkit::keymap::Overrides) {
        if keymap == self.keymap {
            return;
        }
        for note in self.commands.borrow_mut().rebind(&keymap) {
            eprintln!("screenshot: keymap: {note}");
        }
        self.keymap = keymap;
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
                .raw_within(read.service, read.verb, read.body.to_string(), LOOK_BUDGET)
                .await;
            Event::Look(read.ticket, result)
        });
    }

    fn looked(&mut self, ticket: u64, result: Result<Reply, CallError>) {
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
                custom_source,
                keymaps,
                ..
            }) => {
                self.look = (!custom_source).then_some(*appearance);
                self.set_base();
                self.set_keymap(keymaps.get(SERVICE).cloned().unwrap_or_default());
            }
            Some(settings::follow::Followed::Missing) if self.look.is_some() => {
                self.look = None;
                self.set_base();
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

    /// One frame's logic; it runs even while the window is hidden.
    pub fn logic(&mut self, ctx: &egui::Context) {
        for answer in drive::logic(ctx) {
            self.bus.reply(answer.id, answer.rc, answer.body);
        }
        self.pump();
        self.follow_theme();
        self.follow_preview();
        if ctx.input(|i| i.viewport().close_requested()) && !self.exiting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.app.quit();
            self.settle();
        }
    }

    /// One frame's drawing and interaction.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        // Shortcuts are off while a text field has the keyboard, a menu is
        // open, or a dialog is up.
        let typing =
            ui.ctx().memory(|m| m.focused().is_some()) && ui.ctx().egui_wants_keyboard_input();
        let mut events: Vec<UiEvent> = Vec::new();
        if !typing && !toolkit::menu::is_open(ui.ctx()) && !self.app.modal() {
            let fired = self.commands.borrow().shortcuts(ui.ctx(), &self.app);
            let cancelled = fired.contains(&"capture.cancel");
            events.extend(
                fired
                    .into_iter()
                    .map(|id| UiEvent::Command(id, Origin::Shortcut)),
            );
            // Escape cancels a capture (its shortcut, taken above when one
            // runs); only otherwise does it drop the selection.
            if !cancelled
                && !self.app.capturing()
                && self.app.ui.selected.is_some()
                && ui.input(|i| i.key_pressed(egui::Key::Escape))
            {
                events.push(UiEvent::Command("edit.deselect", Origin::Shortcut));
            }
        }
        events.extend(view(
            ui,
            &self.app,
            &self.commands.borrow(),
            &self.strings,
            self.preview.as_ref(),
            &mut self.drag,
        ));
        apply(&mut self.app, &self.commands.borrow(), events);
        self.settle();
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
    fn on_exit(&mut self) {
        toolkit::macros::close_app(&self.ctx, "screenshot");
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        Shell::logic(self, ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        Shell::ui(self, ui);
    }
}
