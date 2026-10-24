// SPDX-License-Identifier: MIT OR Apache-2.0
//! Per-window macro menu, editor and background runtime.
mod notifications;
use crate::menu::{self, Entry as MenuEntry, Menu, Row};
use crate::strings::{own, own_with};
#[cfg(test)]
use ::macros::Runner;
use ::macros::{Catalogue, Entry, Roots, Status, Tier};
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const SLOTS: [&str; 15] = [
    "macro.slot.0",
    "macro.slot.1",
    "macro.slot.2",
    "macro.slot.3",
    "macro.slot.4",
    "macro.slot.5",
    "macro.slot.6",
    "macro.slot.7",
    "macro.slot.8",
    "macro.slot.9",
    "macro.slot.10",
    "macro.slot.11",
    "macro.slot.12",
    "macro.slot.13",
    "macro.slot.14",
];
type Completion = Result<
    (Catalogue, Option<editor::local::Document>, Option<Entry>),
    (String, Option<editor::local::Document>),
>;
type Work = Box<
    dyn FnOnce(&Roots, &str) -> io::Result<(Option<editor::local::Document>, Option<Entry>)> + Send,
>;
type Creation = Result<(Option<Catalogue>, editor::local::Document, Option<String>), String>;
type SaveCompletion = Result<
    (
        editor::local::Saved,
        Option<Catalogue>,
        Option<Entry>,
        Option<String>,
    ),
    String,
>;
static NEXT_WINDOW: AtomicU64 = AtomicU64::new(1);

pub fn command<S>(enabled: fn(&S) -> bool, run: fn(&mut S)) -> crate::Command<S> {
    crate::Command {
        id: "help.macros",
        label: "toolkit-macros",
        menu: Some("help"),
        submenu: None,
        group: 0,
        shortcut: None,
        icon: Some(crate::Icon::ScrollText),
        enabled,
        run,
    }
}

#[derive(Clone)]
struct Finished {
    title: String,
    output: String,
    status: String,
}

/// Runtime resources stay outside app engines and serialised app state.
pub struct State {
    pub open: bool,
    pub engine: ::macros::Session,
    // Window-local row mapping, refreshed from the authoritative Session catalogue.
    catalogue: Catalogue,
    app: String,
    finish_rx: mpsc::Receiver<serde_json::Value>,
    subscriber: u64,
    _change_hook: ::macros::session::ChangeHook,
    repaint: Arc<Mutex<Option<(egui::Context, egui::ViewportId)>>>,
    pending: Option<mpsc::Receiver<Completion>>,
    queued: VecDeque<Work>,
    notifications: mpsc::Receiver<Finished>,
    notify_tx: mpsc::Sender<Finished>,
    lifetime: Arc<()>,
    window: u64,
    title: Option<String>,
    creation: Option<mpsc::Receiver<Creation>>,
    creation_error: Option<String>,
    run_message: Option<String>,
    save_request: Option<bool>,
    saving: Option<mpsc::Receiver<SaveCompletion>>,
    close_after_save: bool,
    loaded: bool,
    error: Option<String>,
    document: Option<editor::local::Document>,
    editor_entry: Option<Entry>,
    editor_open: bool,
    editor_owner: Option<egui::ViewportId>,
    editor_focus: Option<egui::Id>,
    retained: Option<Finished>,
}

impl State {
    #[cfg(test)]
    fn set_test_roots(&mut self, roots: Roots) {
        if self
            .engine
            .roots()
            .is_some_and(|old| old.user == roots.user && old.builtin == roots.builtin)
        {
            return;
        }
        self.engine.remove_finish_hook(self.subscriber);
        let session = ::macros::Session::with_roots(&self.app, Some(roots));
        let (tx, rx) = mpsc::channel();
        self.subscriber = session.add_finish_hook(Arc::new(move |body| {
            let _ = tx.send(body);
        }));
        self.finish_rx = rx;
        self.engine = session;
        self.engine.observe_changes(&self._change_hook);
        let _ = self.engine.reload();
    }

    pub fn new(app: &str) -> Self {
        let (notify_tx, notifications) = mpsc::channel();
        let session = ::macros::Session::for_app(app);
        #[cfg(test)]
        let session = {
            let _ = session;
            ::macros::Session::with_roots(app, Roots::resolve())
        };
        Self::with_session(app, session, notify_tx, notifications)
    }

    fn with_session(
        app: &str,
        session: ::macros::Session,
        notify_tx: mpsc::Sender<Finished>,
        notifications: mpsc::Receiver<Finished>,
    ) -> Self {
        let (finish_tx, finish_rx) = mpsc::channel();
        let lifetime = Arc::new(());
        let weak = Arc::downgrade(&lifetime);
        let window = NEXT_WINDOW.fetch_add(1, Ordering::Relaxed);
        let origin = format!("gui:{window}");
        let subscriber = session.add_finish_hook(Arc::new(move |body| {
            if weak.upgrade().is_some() && body["origin"].as_str() == Some(origin.as_str()) {
                let _ = finish_tx.send(body);
            }
        }));
        let repaint = Arc::new(Mutex::new(None::<(egui::Context, egui::ViewportId)>));
        let target = repaint.clone();
        let change_hook: ::macros::session::ChangeHook = Arc::new(move || {
            if let Some((ctx, viewport)) = &*target.lock().unwrap() {
                ctx.request_repaint_of(*viewport);
            }
        });
        session.observe_changes(&change_hook);
        Self {
            open: false,
            engine: session,
            catalogue: Catalogue::default(),
            finish_rx,
            subscriber,
            _change_hook: change_hook,
            repaint,
            app: app.into(),
            pending: None,
            notifications,
            notify_tx,
            queued: VecDeque::new(),
            lifetime,
            window,
            title: None,
            creation: None,
            creation_error: None,
            run_message: None,
            save_request: None,
            saving: None,
            close_after_save: false,
            loaded: false,
            error: None,
            document: None,
            editor_entry: None,
            editor_open: false,
            editor_owner: None,
            editor_focus: None,
            retained: None,
        }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }
    pub fn reload(&mut self) {
        self.work(|_, _| Ok((None, None)));
    }

    fn work(
        &mut self,
        task: impl FnOnce(&Roots, &str) -> io::Result<(Option<editor::local::Document>, Option<Entry>)>
        + Send
        + 'static,
    ) {
        if self.pending.is_some() {
            self.queued.push_back(Box::new(task));
            return;
        }
        let Some(roots) = self.engine.roots() else {
            self.error = Some(own("macros-no-home"));
            self.open = true;
            return;
        };
        let app = self.app.clone();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.error = None;
        let session = self.engine.clone();
        std::thread::spawn(move || {
            let result = task(&roots, &app)
                .and_then(|(doc, run)| Ok((session.reload().map_err(io::Error::other)?, doc, run)))
                .map_err(|e| (e.to_string(), None));
            let _ = tx.send(result);
        });
    }

    fn run(&mut self, entry: Entry, ctx: &egui::Context) {
        if self.engine.runner_status() == Status::Running {
            self.run_error(&entry.title, own("macros-already-running"), ctx);
            return;
        }
        // Completion can race a menu click between frames. Retain and report
        // the previous terminal run before Runner resets its output buffer.
        self.finish_run(ctx);
        self.run_message = None;
        match self.engine.run(&entry.id, &format!("gui:{}", self.window)) {
            Ok(_) => self.retained = None,
            Err(error) => {
                self.run_error(&entry.title, error.to_string(), ctx);
            }
        }
    }

    fn run_error(&mut self, title: &str, error: String, ctx: &egui::Context) {
        self.run_message = Some(error.clone());
        let payload = notification(&self.app, title, &Status::Failed(error.clone()), &error);
        notifications::post(
            &self.app,
            payload,
            Finished {
                title: title.into(),
                output: error,
                status: own("macros-failed-short"),
            },
            self.notify_tx.clone(),
            &self.lifetime,
            ctx,
        );
    }

    fn finish_run(&mut self, ctx: &egui::Context) {
        while let Ok(body) = self.finish_rx.try_recv() {
            if body["origin"].as_str() != Some(format!("gui:{}", self.window).as_str()) {
                continue;
            }
            self.run_message = None;
            let title = body["title"].as_str().unwrap_or_default();
            let output = body["text"].as_str().unwrap_or_default().to_owned();
            let status = match body["state"].as_str() {
                Some("exited") => Status::Exited(body["exit"].as_i64().map(|v| v as i32)),
                Some("failed_to_start") => {
                    Status::FailedToStart(body["error"].as_str().unwrap_or_default().into())
                }
                _ => Status::Failed(body["error"].as_str().unwrap_or_default().into()),
            };
            let payload = notification(&self.app, title, &status, &output);
            let finished = Finished {
                title: title.into(),
                output,
                status: payload["summary"].as_str().unwrap_or_default().into(),
            };
            self.retained = Some(finished.clone());
            notifications::post(
                &self.app,
                payload,
                finished,
                self.notify_tx.clone(),
                &self.lifetime,
                ctx,
            );
        }
    }

    fn running_title(&self) -> Option<String> {
        let status = self.engine.status();
        (status["state"] == "running")
            .then(|| status["title"].as_str().unwrap_or_default().to_owned())
    }

    fn create_title(&mut self) {
        if self.creation.is_some() {
            return;
        }
        let Some(title) = self.title.clone() else {
            return;
        };
        if self.engine.roots().is_none() {
            self.creation_error = Some(own("macros-no-home"));
            return;
        }
        self.creation_error = None;
        let (tx, rx) = mpsc::channel();
        self.creation = Some(rx);
        let session = self.engine.clone();
        let app = self.app.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let roots = session.roots().expect("creation roots checked");
                let folder = roots.folder(&app, Tier::UserApp)?;
                let answer = session
                    .create(&title)
                    .map_err(|error| path_error("macros-create-error", &folder, error))?;
                let path = answer["path"].as_str().expect("created path is a string");
                let doc = open_macro(&roots, Path::new(path))?;
                let warning = answer["warning"].as_str().map(str::to_owned);
                Ok((session.catalogue(), doc, warning))
            })()
            .map_err(|e: io::Error| e.to_string());
            let _ = tx.send(result);
        });
    }

    fn created(&mut self, result: Creation, ctx: &egui::Context) {
        let viewport = ctx.viewport_id();
        self.creation = None;
        match result {
            Ok((catalogue, doc, warning)) => {
                if let Some(catalogue) = catalogue {
                    self.replace_catalogue(catalogue, ctx);
                }
                self.editor_entry = self
                    .catalogue
                    .entries
                    .iter()
                    .find(|e| Some(e.path.as_path()) == doc.path())
                    .cloned();
                self.document = Some(doc);
                self.editor_open = true;
                self.open = false;
                self.title = None;
                self.creation_error = None;
                // Any older discovery still in flight must not win over the
                // new file; queue a fresh scan behind it.
                self.reload();
                self.error = warning;
            }
            Err(error) => {
                self.creation_error = Some(error);
                self.open = true;
                ctx.data_mut(|d| {
                    d.insert_temp(crate::window_id(viewport, "macro-title-focused"), false)
                });
            }
        }
    }

    fn edit(&mut self, entry: Entry) {
        if !self.can_replace_document() {
            return;
        }
        self.work(move |roots, app| {
            let path = if entry.tier.is_user() {
                roots.read(&entry)?;
                entry.path
            } else {
                let tier = if entry.tier.is_app() {
                    Tier::UserApp
                } else {
                    Tier::UserGlobal
                };
                let destination = roots.folder(app, tier)?.join(format!("{}.mix", entry.id));
                roots
                    .copy_to_user(app, &entry)
                    .map_err(|e| path_error("macros-create-error", &destination, e))?
            };
            Ok((Some(open_macro(roots, &path)?), None))
        });
    }

    fn can_replace_document(&mut self) -> bool {
        if self.saving.is_some()
            || self.close_after_save
            || self.document.as_ref().is_some_and(|doc| doc.is_dirty())
        {
            self.error = Some(own("macros-save-first"));
            false
        } else {
            true
        }
    }

    fn folder(&mut self) {
        self.work(|roots, app| {
            let workshop = Path::new("/opt/mixos/bin/workshop");
            if workshop.is_file() {
                let mut child = Command::new(workshop).args(["--app", app])
                    .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
                std::thread::spawn(move || { let _ = child.wait(); });
                return Ok((None, None));
            }
            launch(&roots.ensure_user_folder(app, Tier::UserGlobal)?, true)?;
            Ok((None, None))
        });
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        *self.repaint.lock().unwrap() = Some((ctx.clone(), ctx.viewport_id()));
        let viewport = ctx.viewport_id();
        if let Some(rx) = &self.saving {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(own("macros-worker-failed"))),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.saved(result, ctx);
            }
        }
        if let Some(rx) = &self.creation {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(own("macros-worker-failed"))),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.created(result, ctx);
            }
        }
        if let Some(rx) = &self.pending {
            let completion = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err((own("macros-worker-failed"), None)))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = completion {
                self.pending = None;
                self.loaded = true;
                match result {
                    Ok((catalogue, doc, run)) => {
                        self.replace_catalogue(catalogue, ctx);
                        if let Some(doc) = doc {
                            self.editor_entry = self
                                .catalogue
                                .entries
                                .iter()
                                .find(|e| Some(e.path.as_path()) == doc.path())
                                .cloned();
                            self.document = Some(doc);
                            self.editor_open = true;
                        }
                        if let Some(entry) = run {
                            let entry = self
                                .catalogue
                                .entries
                                .iter()
                                .find(|e| e.path == entry.path)
                                .cloned()
                                .unwrap_or(entry);
                            self.run(entry, ctx);
                        }
                    }
                    Err((error, doc)) => {
                        if let Some(doc) = doc {
                            self.document = Some(doc);
                        }
                        self.error = Some(error);
                        self.open = true;
                    }
                }
            }
        }
        if self.pending.is_none()
            && let Some(task) = self.queued.pop_front()
        {
            self.work(task);
        }
        if let Some(catalogue) = self.engine.catalogue() {
            self.sync_catalogue(catalogue, ctx);
        }
        self.finish_run(ctx);
        while let Ok(finished) = self.notifications.try_recv() {
            if self.editor_open {
                self.retained = Some(finished);
            } else {
                eprintln!(
                    "{}: {} (notification unavailable)",
                    self.app, finished.status
                );
            }
        }
        let busy = self.engine.runner_status() == Status::Running;
        let changed = ctx.data_mut(|d| {
            let id = crate::window_id(viewport, "macro-busy");
            let changed = d.get_temp::<bool>(id).unwrap_or(false) != busy;
            d.insert_temp(id, busy);
            changed
        });
        if changed {
            ctx.request_repaint();
        }
        if self.open && !self.loaded && self.pending.is_none() {
            self.reload();
        }
        let anchor = ctx
            .data(|d| d.get_temp::<egui::Rect>(crate::window_id(viewport, "macro-anchor")))
            .unwrap_or_else(|| {
                egui::Rect::from_min_size(ctx.content_rect().left_top(), egui::Vec2::ZERO)
            });
        let mut selected = None;
        let running_title = self.running_title();
        egui::Area::new(crate::window_id(viewport, "macro-menu-owner"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor.left_bottom())
            .show(ctx, |ui| {
                if let Some(title) = &mut self.title {
                    if self.open {
                        menu::anchored_open(ctx, true);
                        let chrome = crate::chrome::Chrome::of(ctx);
                        let (mut model, _) = macro_menu(&self.catalogue, running_title.as_deref());
                        append_menu_error(&mut model, self.error.as_deref());
                        let menu_rect = menu::anchored_menu_rect(ui, anchor, &model);
                        let margin = egui::Frame::menu(ui.style()).total_margin().sum();
                        let width = (menu_rect.width() - margin.x).max(0.0);
                        let height = chrome
                            .metrics
                            .menu_row_height
                            .max(ui.spacing().interact_size.y);
                        let error_height = self.creation_error.as_ref().map_or(0.0, |error| {
                            ui.spacing().item_spacing.y
                                + ui.painter()
                                    .layout(
                                        error.clone(),
                                        egui::TextStyle::Body.resolve(ui.style()),
                                        ui.visuals().text_color(),
                                        width,
                                    )
                                    .size()
                                    .y
                        });
                        let bounds = ctx.content_rect().shrink(chrome.metrics.edge_gap);
                        let rect = menu::anchored_rect(
                            anchor,
                            egui::vec2(menu_rect.width(), height + error_height + margin.y),
                            bounds,
                        );
                        let error_limit =
                            (rect.height() - margin.y - height - ui.spacing().item_spacing.y)
                                .max(0.0);
                        let shown =
                            egui::Area::new(crate::window_id(viewport, "macro-title-owner"))
                                .kind(egui::UiKind::Menu)
                                .order(egui::Order::Foreground)
                                .fixed_pos(rect.min)
                                .default_size(rect.size())
                                .constrain(false)
                                .fade_in(false)
                                .show(ctx, |ui| {
                                    egui::Frame::menu(ui.style()).show(ui, |ui| {
                                        ui.set_width(width);
                                        // Areas cache their last size. Grow the content
                                        // before allocating a newly wrapped error strip.
                                        ui.set_min_height((rect.height() - margin.y).max(0.0));
                                        let (enter, escape) = menu::title_keys(ctx);
                                        let field = ui.add_enabled(
                                            self.creation.is_none(),
                                            egui::TextEdit::singleline(title)
                                                .desired_width(width)
                                                .min_size(egui::vec2(width, height))
                                                .id(crate::window_id(viewport, "macro-title"))
                                                .hint_text(own("macros-title")),
                                        );
                                        if let Some(error) = &self.creation_error {
                                            egui::ScrollArea::vertical()
                                                .max_width(width)
                                                .max_height(error_limit)
                                                .min_scrolled_height(0.0)
                                                .show(ui, |ui| {
                                                    ui.add(egui::Label::new(error).wrap())
                                                });
                                        }
                                        // Only a visible, enabled widget has an AccessKit
                                        // node to receive focus. Areas may first run an
                                        // invisible sizing pass; leave the request pending.
                                        if ui.is_visible()
                                            && field.enabled()
                                            && !ctx
                                                .data(|d| {
                                                    d.get_temp::<bool>(crate::window_id(
                                                        viewport,
                                                        "macro-title-focused",
                                                    ))
                                                })
                                                .unwrap_or(false)
                                        {
                                            field.request_focus();
                                            ctx.data_mut(|d| {
                                                d.insert_temp(
                                                    crate::window_id(
                                                        viewport,
                                                        "macro-title-focused",
                                                    ),
                                                    true,
                                                )
                                            });
                                        }
                                        (field, enter, escape)
                                    })
                                });
                        menu::title_bounds(ctx, shown.response.rect);
                        let (field, enter, escape) = shown.inner.inner;
                        let outside = ctx.input(|i| {
                            i.pointer.primary_pressed()
                                && i.pointer
                                    .interact_pos()
                                    .is_some_and(|p| !shown.response.rect.contains(p))
                        });
                        if enter && !title.trim().is_empty() {
                            // The field becomes disabled while creation runs.
                            // Release focus while its node is still drawn.
                            field.surrender_focus();
                            selected = Some(menu::Pick {
                                id: "macro.create",
                                edit: false,
                            });
                        }
                        if escape || outside {
                            field.surrender_focus();
                            self.open = false;
                        }
                    }
                } else {
                    let (mut model, indices) =
                        macro_menu(&self.catalogue, running_title.as_deref());
                    append_menu_error(&mut model, self.error.as_deref());
                    let tips = SLOTS
                        .iter()
                        .map(|&id| (id, own("macros-edit-tip")))
                        .collect::<Vec<_>>();
                    if let Some(pick) = menu::anchored(
                        ui,
                        crate::window_id(viewport, "macro-nav"),
                        anchor,
                        &mut self.open,
                        model,
                        &tips,
                    ) {
                        if let Some(slot) = SLOTS.iter().position(|id| *id == pick.id) {
                            let entry = self.catalogue.entries[indices[slot]].clone();
                            if pick.edit {
                                self.edit(entry);
                            } else {
                                self.run(entry, ctx);
                            }
                        } else {
                            selected = Some(pick);
                        }
                    }
                }
            });
        if let Some(pick) = selected {
            match pick.id {
                "macro.new" => {
                    if self.can_replace_document() {
                        self.title = Some(String::new());
                        self.creation_error = None;
                        self.open = true;
                        menu::open_title(ctx);
                        // The title widget is drawn on the next frame. Its
                        // response requests focus then, never on a missing node.
                        ctx.data_mut(|d| {
                            d.insert_temp(crate::window_id(viewport, "macro-title-focused"), false)
                        });
                        ctx.request_repaint();
                    }
                }
                "macro.create" => {
                    self.create_title();
                    ctx.request_repaint();
                }
                "macro.workshop" => self.folder(),
                "macro.stop" => {
                    self.engine.stop();
                }
                _ => {}
            }
        }
        if !self.open {
            menu::close_title(ctx);
            ctx.memory_mut(|m| m.surrender_focus(crate::window_id(viewport, "macro-title")));
            self.title = None;
        }
        menu::anchored_open(ctx, self.open);
        self.show_editor(ctx);
        self.save_editor(false, false);
        // Only live workers need polling. Native notification deliveries wake
        // this viewport directly, including when its menu is closed.
        if self.needs_repaint() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn needs_repaint(&self) -> bool {
        self.engine.runner_status() == Status::Running
            || self.pending.is_some()
            || self.creation.is_some()
            || self.saving.is_some()
            || self.save_request.is_some()
    }

    fn show_editor(&mut self, ctx: &egui::Context) {
        let viewport = ctx.viewport_id();
        if !self.editor_open {
            if let Some(owner) = self.editor_owner.take() {
                let editor = egui::ViewportId::from_hash_of(("macro-editor", self.window));
                crate::drive::unregister_viewport(ctx, owner, "macro-editor", editor);
            }
            return;
        }
        let editor_viewport = egui::ViewportId::from_hash_of(("macro-editor", self.window));
        let title = self
            .editor_entry
            .as_ref()
            .map(|e| e.title.as_str())
            .unwrap_or_default();
        let dirty = self.document.as_ref().is_some_and(|doc| doc.is_dirty());
        let title = format!(
            "{}{} — {}",
            title,
            if dirty { " *" } else { "" },
            own("macros-editor")
        );
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_decorations(true)
            .with_resizable(true)
            .with_inner_size(ctx.input(|i| {
                let monitor = i
                    .viewport()
                    .monitor_size
                    .unwrap_or(egui::vec2(760.0, 560.0));
                egui::vec2(760.0, 560.0).min(monitor)
            }));
        if ctx.embed_viewports() {
            ctx.show_viewport_immediate(editor_viewport, builder, |ui, _| {
                self.editor_ui(ui, viewport, editor_viewport)
            });
        } else if let Some(state) = window_state(ctx, &self.app, false) {
            let weak = Arc::downgrade(&state);
            ctx.show_viewport_deferred(editor_viewport, builder, move |ui, _| {
                if let Some(state) = weak.upgrade() {
                    let mut state = state.lock().unwrap();
                    if state.editor_open {
                        state.editor_ui(ui, viewport, editor_viewport);
                    }
                }
            });
            ctx.request_repaint_of(editor_viewport);
        }
    }

    fn editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        viewport: egui::ViewportId,
        editor_viewport: egui::ViewportId,
    ) {
        let ctx = ui.ctx().clone();
        // Embedded editors have no child pass to service Drive requests.
        if !ctx.embed_viewports() && ctx.viewport_id() == editor_viewport {
            crate::drive::register_viewport(&ctx, viewport, "macro-editor", editor_viewport);
            self.editor_owner = Some(viewport);
        }
        let mut save = false;
        let mut run = false;
        let mut ced = false;
        let row_height = crate::chrome::Chrome::of(&ctx).metrics.menu_row_height;
        let output = self.displayed_output();
        let dirty_before = self.document.as_ref().is_some_and(|doc| doc.is_dirty());
        let mut open = !ctx.input(|i| i.viewport().close_requested());
        if ctx.input(|i| i.viewport().focused == Some(true))
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            open = false;
        }
        if !output.is_empty() {
            egui::Panel::bottom("macro-output")
                .exact_size(row_height * 6.0)
                .show(ui, |ui| {
                    if self.engine.runner_status() != Status::Running
                        && let Some(last) = &self.retained
                    {
                        ui.label(&last.status).on_hover_text(&last.title);
                    }
                    if self.engine.status()["origin"].as_str()
                        == Some(format!("gui:{}", self.window).as_str())
                        && let Some(title) = self.running_title()
                    {
                        ui.label(own_with("macros-running-title", &[("title", &title)]));
                    }
                    egui::ScrollArea::both()
                        .auto_shrink(false)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(output).monospace())
                                    .selectable(true),
                            );
                        });
                });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            let ctx = &ui.ctx().clone();
            if self
                .editor_focus
                .is_some_and(|id| ctx.memory(|m| m.has_focus(id)))
                && !menu::is_open(ctx)
            {
                save = ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S));
                run = ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Enter));
            }
            save |= ctx
                .data_mut(|d| d.remove_temp::<bool>(crate::window_id(viewport, "macro-save")))
                .unwrap_or(false);
            run |= ctx
                .data_mut(|d| d.remove_temp::<bool>(crate::window_id(viewport, "macro-save-run")))
                .unwrap_or(false);
            let editor_enabled = !menu::is_open(ctx);
            if let Some(error) = &self.error {
                ui.label(error);
            }
            if let Some(message) = &self.run_message {
                ui.label(message);
            }
            if let Some(doc) = &mut self.document {
                ui.horizontal(|ui| {
                    save |= ui.button(own("macros-save")).clicked();
                    run |= ui.button(own("macros-run")).clicked();
                    ced = ui.button(own("macros-open-ced")).clicked();
                    if doc.is_dirty() {
                        ui.label(own("macros-unsaved"));
                    }
                });
                ui.add(
                    egui::Label::new(
                        doc.path()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                    )
                    .wrap(),
                );
                let palette = ctx
                    .data(|d| {
                        d.get_temp::<editor::Palette>(egui::Id::new("toolkit.editor-palette"))
                    })
                    .unwrap_or_else(|| editor::Palette::from_theme(&crate::Theme::embedded()));
                let height = ui.available_height().max(0.0);
                ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                    ui.add_enabled_ui(editor_enabled, |ui| {
                        self.editor_focus =
                            Some(doc.show(ui, &palette, &editor::View::default()).response.id);
                        ctx.data_mut(|d| {
                            d.insert_temp(
                                crate::window_id(viewport, "macro-editor-focus"),
                                self.editor_focus,
                            )
                        });
                    });
                });
                if let Some(error) = doc.take_refusal() {
                    self.error = Some(error.to_string());
                }
            }
        });
        if !open {
            self.close_editor();
            if self.editor_open {
                ctx.send_viewport_cmd_to(editor_viewport, egui::ViewportCommand::CancelClose);
            }
        }
        if !self.editor_open {
            if let Some(id) = self.editor_focus {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
            ctx.data_mut(|d| {
                d.remove::<Option<egui::Id>>(crate::window_id(viewport, "macro-editor-focus"))
            });
        }
        if ced
            && let Some(path) = self
                .document
                .as_ref()
                .and_then(|d| d.path())
                .map(Path::to_owned)
        {
            self.work(move |_, _| {
                launch(&path, false)?;
                Ok((None, None))
            });
        }
        self.save_editor(save, run);
        if !open
            || save
            || run
            || ced
            || dirty_before != self.document.as_ref().is_some_and(|doc| doc.is_dirty())
        {
            ctx.request_repaint_of(viewport);
        }
    }

    fn save_editor(&mut self, save: bool, mut run: bool) {
        let requested = run;
        if run && self.engine.runner_status() == Status::Running {
            self.run_message = Some(own("macros-already-running"));
            run = false;
        }
        if save || requested {
            self.save_request = Some(self.save_request.unwrap_or(false) || run);
        }
        if self.saving.is_none()
            && self.document.is_some()
            && let Some(run) = self.save_request.take()
        {
            let path = self
                .document
                .as_ref()
                .unwrap()
                .path()
                .map(Path::to_owned)
                .unwrap_or_default();
            let snapshot = match self.document.as_ref().unwrap().save_snapshot() {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    self.error = Some(path_error("macros-save-error", &path, error).to_string());
                    self.close_after_save = false;
                    return;
                }
            };
            let entry = if run { self.editor_entry.clone() } else { None };
            let session = self.engine.clone();
            let (tx, rx) = mpsc::channel();
            self.saving = Some(rx);
            std::thread::spawn(move || {
                let mut warning = None;
                let completion = snapshot
                    .write_with(|path, bytes| {
                        session
                            .roots()
                            .ok_or_else(|| io::Error::other("macro roots unavailable"))?
                            .write_path_outcome(path, bytes)
                            .map(|outcome| warning = outcome.warning)
                    })
                    .map(|saved| {
                        let catalogue = session.reload().ok();
                        (saved, catalogue, entry, warning)
                    })
                    .map_err(|error| path_error("macros-save-error", &path, error).to_string());
                let _ = tx.send(completion);
            });
        }
    }

    fn saved(&mut self, result: SaveCompletion, ctx: &egui::Context) {
        self.saving = None;
        match result {
            Ok((saved, catalogue, run, warning)) => {
                if let Some(catalogue) = catalogue {
                    self.replace_catalogue(catalogue, ctx);
                    self.reload();
                }
                if let Some(doc) = &mut self.document {
                    doc.complete_save(&saved);
                }
                self.error = None;
                if let Some(entry) = run {
                    self.run(entry, ctx);
                }
                if warning.is_some() {
                    self.error = warning;
                }
                if self.close_after_save {
                    if self.document.as_ref().is_some_and(|doc| doc.is_dirty())
                        || self.save_request.is_some()
                    {
                        self.save_editor(true, false);
                    } else {
                        self.editor_open = false;
                        self.close_after_save = false;
                    }
                }
            }
            Err(error) => {
                self.error = Some(error);
                self.close_after_save = false;
            }
        }
    }

    fn close_editor(&mut self) {
        if self.document.as_ref().is_some_and(|doc| doc.is_dirty())
            || self.saving.is_some()
            || self.save_request.is_some()
        {
            self.close_after_save = true;
            if self.saving.is_none() && self.save_request.is_none() {
                self.save_editor(true, false);
            }
        } else {
            self.editor_open = false;
        }
    }

    fn displayed_output(&self) -> String {
        if self.engine.runner_status() == Status::Running
            && self.engine.status()["origin"].as_str()
                == Some(format!("gui:{}", self.window).as_str())
        {
            self.engine.runner_output().text
        } else {
            self.retained
                .as_ref()
                .map(|r| r.output.clone())
                .unwrap_or_default()
        }
    }

    fn replace_catalogue(&mut self, catalogue: Catalogue, ctx: &egui::Context) {
        // A receipt may be older than a Bus scan completed in the meantime.
        self.sync_catalogue(self.engine.catalogue().unwrap_or(catalogue), ctx);
    }

    fn sync_catalogue(&mut self, catalogue: Catalogue, ctx: &egui::Context) {
        // Cached keyboard picks refer to the old row-to-file mapping. Closing
        // invalidates both the highlight and any already queued activation.
        if !self.catalogue.entries.is_empty() && self.catalogue.entries != catalogue.entries {
            menu::dismiss_anchored(ctx);
            if self.title.is_none() {
                self.open = false;
            }
        }
        self.catalogue = catalogue;
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.engine.stop_if_origin(&format!("gui:{}", self.window));
        self.engine.remove_finish_hook(self.subscriber);
    }
}

pub(crate) fn keyboard(ctx: &egui::Context) {
    let viewport = ctx.viewport_id();
    let focus = ctx
        .data(|d| d.get_temp::<Option<egui::Id>>(crate::window_id(viewport, "macro-editor-focus")))
        .flatten();
    if focus.is_some_and(|id| ctx.memory(|m| m.has_focus(id))) && !menu::is_open(ctx) {
        for (key, flag) in [
            (egui::Key::S, "macro-save"),
            (egui::Key::Enter, "macro-save-run"),
        ] {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, key)) {
                ctx.data_mut(|d| d.insert_temp(crate::window_id(viewport, flag), true));
            }
        }
    }
}

fn append_menu_error(menu: &mut Menu, error: Option<&str>) {
    if let Some(error) = error {
        menu.entries.push(MenuEntry::Separator);
        menu.entries.push(MenuEntry::Row(Row::command(
            "macro.error",
            error,
            None,
            false,
        )));
    }
}

fn macro_menu(catalogue: &Catalogue, running: Option<&str>) -> (Menu, Vec<usize>) {
    let row = |id, label| MenuEntry::Row(Row::command(id, label, None, true));
    let mut entries = Vec::new();
    if let Some(title) = running {
        entries.push(row(
            "macro.stop",
            own_with("macros-stop-title", &[("title", title)]),
        ));
    }
    entries.push(row("macro.new", own("macros-new")));
    entries.push(row("macro.workshop", own("macros-workshop")));
    let mut indices = Vec::new();
    if catalogue.entries.is_empty() {
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::Row(Row::command(
            "macro.empty",
            own("macros-empty"),
            None,
            false,
        )));
    }
    {
        entries.push(MenuEntry::Separator);
        for (index, entry) in catalogue
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.tier.is_app())
        {
            if indices.len() == SLOTS.len() {
                continue;
            }
            let mut item =
                Row::command(SLOTS[indices.len()], &entry.title, None, running.is_none());
            if !entry.tier.is_user() {
                item.shortcut = Some(own("macros-builtin-mark"));
            }
            entries.push(MenuEntry::Row(item));
            indices.push(index);
        }
    }
    if catalogue.entries.len() > indices.len() {
        entries.push(row("macro.workshop", own("macros-more")));
    }
    (
        Menu {
            title: own("toolkit-macros"),
            entries: menu::tidy(entries),
        },
        indices,
    )
}

fn notification(app: &str, title: &str, status: &Status, output: &str) -> serde_json::Value {
    let result = match status {
        Status::Exited(Some(0)) => own("macros-done"),
        Status::Exited(Some(code)) => {
            own_with("macros-exit-failed", &[("code", &code.to_string())])
        }
        _ => own("macros-failed-short"),
    };
    let lines: Vec<_> = output.lines().rev().take(5).collect();
    let body = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
    let body = if body.is_empty() {
        match status {
            Status::FailedToStart(error) | Status::Failed(error) => error.clone(),
            _ => result.clone(),
        }
    } else {
        body
    };
    let title = title
        .chars()
        .take(119usize.saturating_sub(result.chars().count()))
        .collect::<String>();
    serde_json::json!({
        "summary": format!("{title} {result}"),
        "body": body.chars().rev().take(500).collect::<Vec<_>>().into_iter().rev().collect::<String>(),
        "app": app, "actions": [], "timeout_ms": 5000
    })
}

fn path_error(key: &str, path: &Path, error: impl std::fmt::Display) -> io::Error {
    io::Error::other(own_with(
        key,
        &[
            ("path", &path.display().to_string()),
            ("error", &error.to_string()),
        ],
    ))
}

/// Read an existing macro and check the directory used by atomic saves before
/// offering it for editing. Probe only a new, exclusively-created scratch file;
/// never truncate or rewrite the macro to test writability.
fn open_macro(roots: &::macros::Roots, path: &Path) -> io::Result<editor::local::Document> {
    ::macros::check_editable(path).map_err(|e| path_error("macros-save-error", path, e))?;
    // The read also refuses links at every component; preflight is no authority.
    let bytes = roots
        .read_path(path)
        .map_err(|e| path_error("macros-open-error", path, e))?;
    let document = editor::local::Document::open_bytes(path, &bytes)
        .map_err(|e| path_error("macros-open-error", path, e))?;
    Ok(document)
}

fn launch(path: &Path, folder: bool) -> io::Result<()> {
    let native = if folder {
        "/opt/mixos/bin/dopus"
    } else {
        "/opt/mixos/bin/ced"
    };
    let mut child = Command::new(if Path::new(native).is_file() {
        native
    } else {
        "xdg-open"
    })
    .arg(path)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn show_app(ctx: &egui::Context, app: &str, open: bool) -> bool {
    // Even an unopened menu observes Bus runs and displays the busy dot.
    let Some(state) = window_state(ctx, app, true) else {
        return false;
    };
    let mut state = state.lock().unwrap();
    state.open = open;
    state.show(ctx);
    state.open != open
}

type WindowState = Arc<Mutex<State>>;

/// Release native-window resources on actual app exit, including when a
/// notification still owns a Context for repaint delivery. A close request
/// alone is insufficient: applications can cancel it for unsaved documents.
pub fn close_app(ctx: &egui::Context, app: &str) {
    let closing = ctx.viewport_id();
    let state_id = crate::window_id(closing, ("shared-macros", app));
    if let Some(state) = ctx.data(|d| d.get_temp::<WindowState>(state_id)) {
        let mut state = state.lock().unwrap();
        if let Some(owner) = state.editor_owner.take() {
            let editor = egui::ViewportId::from_hash_of(("macro-editor", state.window));
            crate::drive::unregister_viewport(ctx, owner, "macro-editor", editor);
        }
    }
    let owners_id = egui::Id::new("macro-viewport-owners");
    ctx.data_mut(|d| {
        let mut owners = d
            .get_temp::<Vec<(egui::ViewportId, egui::Id)>>(owners_id)
            .unwrap_or_default();
        owners.retain(|(viewport, id)| {
            let keep = *viewport != closing || *id != state_id;
            if !keep {
                d.remove::<WindowState>(*id);
            }
            keep
        });
        d.insert_temp(owners_id, owners);
    });
}

fn window_state(ctx: &egui::Context, app: &str, open: bool) -> Option<WindowState> {
    let viewport = ctx.viewport_id();
    let owners_id = egui::Id::new("macro-viewport-owners");
    let mut owners = ctx
        .data(|d| d.get_temp::<Vec<(egui::ViewportId, egui::Id)>>(owners_id))
        .unwrap_or_default();
    owners.retain(|(viewport, id)| {
        let present = *viewport == egui::ViewportId::ROOT
            || ctx.input(|i| i.raw.viewports.contains_key(viewport));
        if !present {
            ctx.data_mut(|d| d.remove::<WindowState>(*id));
        }
        present
    });
    let id = crate::window_id(viewport, ("shared-macros", app));
    let state: Option<WindowState> = ctx.data(|d| d.get_temp(id));
    if !open && state.is_none() {
        ctx.data_mut(|d| d.insert_temp(owners_id, owners));
        return None;
    }
    let state = state.unwrap_or_else(|| {
        let state = Arc::new(Mutex::new(State::new(app)));
        ctx.data_mut(|d| d.insert_temp(id, state.clone()));
        owners.push((ctx.viewport_id(), id));
        state
    });
    ctx.data_mut(|d| d.insert_temp(owners_id, owners));
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn notification_fallback_and_failed_save_never_reopen_a_closed_editor() {
        let mut state = State::new("ced");
        state.loaded = true;
        state
            .notify_tx
            .send(Finished {
                title: "Test".into(),
                output: "result".into(),
                status: "done".into(),
            })
            .unwrap();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| state.show(&ctx));
        output.textures_delta.clear();
        assert!(!state.editor_open);
        state.saved(Err("read-only".into()), &ctx);
        assert!(!state.editor_open);
        state.editor_open = true;
        state.saved(Err("read-only".into()), &ctx);
        assert!(state.editor_open);
        assert_eq!(state.error.as_deref(), Some("read-only"));
    }

    #[cfg(unix)]
    fn runner_fixture() -> (tempfile::TempDir, Roots, Entry) {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let mix = temp.path().join("runner-stub");
        // Explicit test-only shell fixture: the runner launches this instead
        // of Mix, so start/stop/reaping never depend on an installed binary.
        std::fs::write(&mix, "#!/bin/sh\n# SPDX-License-Identifier: MIT OR Apache-2.0\nprintf '%s\\n' \"$$\" > \"$1.parent\"\n/bin/sleep 120 &\nchild=$!\nprintf '%s\\n' \"$child\" > \"$1.child\"\nwait \"$child\"\n").unwrap();
        std::fs::set_permissions(&mix, std::fs::Permissions::from_mode(0o700)).unwrap();
        let roots = Roots {
            builtin: temp.path().join("builtin"),
            user: temp.path().join("user"),
            home: temp.path().into(),
            mix,
        };
        let path = roots.create_from_title("ced", "Wait").unwrap();
        let entry = Entry {
            path,
            id: "wait".into(),
            title: "Wait".into(),
            tier: Tier::UserApp,
            shadows: vec![],
            targets: None,
        };
        (temp, roots, entry)
    }

    fn wait_until(mut predicate: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !predicate() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the macro worker"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[cfg(unix)]
    #[test]
    fn closing_finished_gui_window_preserves_the_next_bus_run() {
        let (_temp, roots, entry) = runner_fixture();
        let mut state = State::new("ced");
        state.set_test_roots(roots);
        let session = state.engine.clone();
        let (tx, rx) = mpsc::channel();
        session.add_finish_hook(Arc::new(move |body| {
            let _ = tx.send(body);
        }));
        state.run(entry.clone(), &egui::Context::default());
        assert!(session.stop_if_origin(&format!("gui:{}", state.window)));
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(!session.stop_if_origin(&format!("gui:{}", state.window)));
        session.run(&entry.id, "bus").unwrap();
        drop(state);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        assert_eq!(session.runner_status(), Status::Running);
        session.stop();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
    }

    #[test]
    fn created_editor_opens_with_a_nonfatal_scan_warning() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("created.mix");
        std::fs::write(&path, "-- title: Created\n").unwrap();
        let doc = editor::local::Document::open(&path).unwrap();
        let mut state = State::new("ced");
        state.created(
            Ok((None, doc, Some("forced scan failure".into()))),
            &egui::Context::default(),
        );
        assert!(state.editor_open);
        assert_eq!(
            state.document.as_ref().unwrap().path(),
            Some(path.as_path())
        );
        assert_eq!(state.error.as_deref(), Some("forced scan failure"));
        assert!(state.creation_error.is_none());
        assert!(path.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn state_shares_one_runner_and_stop_ends_it() {
        let (_temp, roots, entry) = runner_fixture();
        let mut state = State::new("ced");
        state.set_test_roots(roots);
        let ctx = egui::Context::default();
        state.run(entry.clone(), &ctx);
        state.run(entry.clone(), &ctx);
        assert_eq!(state.engine.runner_status(), Status::Running);
        assert!(state.run_message.is_some());
        let (tx, rx) = mpsc::channel();
        let other_window = State::with_session("ced", state.engine.clone(), tx, rx);
        assert_eq!(other_window.engine.runner_status(), Status::Running);
        state.document = Some(editor::local::Document::open(&entry.path).unwrap());
        state
            .document
            .as_mut()
            .unwrap()
            .command(
                editor::EditCommand::Insert("-- saved\n".into()),
                &editor::View::default(),
            )
            .unwrap();
        state.save_editor(false, true);
        assert!(state.document.is_some(), "saving must not blank the editor");
        let result = state
            .saving
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(
            matches!(&result, Ok((_, _, None, _))),
            "Ctrl+Enter only saves while already running"
        );
        state.saved(result, &ctx);
        assert!(!state.document.as_ref().unwrap().is_dirty());
        state.engine.stop();
        wait_until(|| state.engine.runner_status() != Status::Running);
        assert!(matches!(state.engine.runner_status(), Status::Exited(_)));
    }

    #[cfg(unix)]
    #[test]
    fn durability_warning_marks_save_complete_and_stays_visible() {
        let (_temp, roots, entry) = runner_fixture();
        let mut state = State::new("ced");
        state.set_test_roots(roots);
        let mut document = editor::local::Document::open(&entry.path).unwrap();
        document
            .command(
                editor::EditCommand::Insert("-- changed\n".into()),
                &editor::View::default(),
            )
            .unwrap();
        let saved = document.save_snapshot().unwrap().write().unwrap();
        state.document = Some(document);
        state.saved(
            Ok((
                saved,
                None,
                Some(entry),
                Some("directory sync failed".into()),
            )),
            &egui::Context::default(),
        );
        assert!(!state.document.as_ref().unwrap().is_dirty());
        assert_eq!(state.error.as_deref(), Some("directory sync failed"));
        state.engine.stop();
        wait_until(|| state.engine.runner_status() != Status::Running);
    }

    #[cfg(unix)]
    #[test]
    fn late_output_does_not_hide_a_live_run() {
        let (_temp, roots, entry) = runner_fixture();
        let mut state = State::new("ced");
        state.set_test_roots(roots);
        state.run(entry, &egui::Context::default());
        state.retained = Some(Finished {
            title: "Earlier run".into(),
            output: "earlier output".into(),
            status: "Earlier run done".into(),
        });
        assert_eq!(state.running_title().unwrap(), "Wait");
        assert_eq!(state.displayed_output(), state.engine.runner_output().text);
        assert_ne!(state.displayed_output(), "earlier output");
        state.engine.stop();
        wait_until(|| state.engine.runner_status() != Status::Running);
        assert_eq!(state.displayed_output(), "earlier output");
    }

    #[cfg(unix)]
    #[test]
    fn bus_and_windows_share_busy_status_but_only_the_origin_retains_a_toast() {
        let (_temp, roots, entry) = runner_fixture();
        let mut first = State::new("ced");
        first.set_test_roots(roots);
        let (tx, rx) = mpsc::channel();
        let mut second = State::with_session("ced", first.engine.clone(), tx, rx);
        let session = first.engine.clone();
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        let removed = session.add_finish_hook(Arc::new(move |body| {
            let _ = tx.send(body);
        }));
        session.run(&entry.id, "bus").unwrap();
        assert_eq!(first.running_title().as_deref(), Some("Wait"));
        assert_eq!(second.running_title().as_deref(), Some("Wait"));
        first.run(entry.clone(), &ctx);
        assert!(first.run_message.is_some());
        session.stop();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        first.finish_run(&ctx);
        second.finish_run(&ctx);
        assert!(first.retained.is_none());
        assert!(second.retained.is_none());
        first.run(entry, &ctx);
        session.stop();
        let event = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        first.finish_run(&ctx);
        second.finish_run(&ctx);
        assert!(first.retained.is_some());
        assert!(second.retained.is_none());
        assert_eq!(event["origin"], format!("gui:{}", first.window));
        session.remove_finish_hook(removed);
        let subscriber = second.subscriber;
        drop(second);
        session.remove_finish_hook(subscriber); // removal is idempotent
    }

    #[cfg(unix)]
    #[test]
    fn stop_during_startup_and_rerun_immediately_after_stop() {
        let (_temp, roots, entry) = runner_fixture();
        let mut runner = Runner::default();
        runner.start(&roots, "ced", &entry).unwrap();
        runner.stop(); // stop may arrive before the manager has spawned.
        let mut restarted = false;
        wait_until(|| match runner.start(&roots, "ced", &entry) {
            Ok(()) => {
                restarted = true;
                true
            }
            Err(error) => {
                assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
                false
            }
        });
        assert!(restarted);
        assert_eq!(runner.status(), Status::Running);
        runner.stop();
        wait_until(|| runner.status() != Status::Running);
        assert!(matches!(runner.status(), Status::Exited(_)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn closing_a_window_kills_its_group_and_reaps_the_leader() {
        let (_temp, roots, entry) = runner_fixture();
        let mut state = State::new("ced");
        state.set_test_roots(roots);
        state.run(entry.clone(), &egui::Context::default());
        let parent_file = entry.path.with_extension("mix.parent");
        let child_file = entry.path.with_extension("mix.child");
        wait_until(|| std::fs::read_to_string(&child_file).is_ok_and(|s| !s.trim().is_empty()));
        let parent: i32 = std::fs::read_to_string(parent_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let child: i32 = std::fs::read_to_string(child_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(state);
        // ESRCH for the leader proves it is no longer even a zombie. Do not
        // waitpid before this: stealing the manager's reap would break its
        // WNOWAIT reservation and could turn group cleanup into a PID race.
        wait_until(|| {
            // SAFETY: signal zero only probes the test-owned process.
            (unsafe { libc::kill(parent, 0) }) == -1
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        });
        let mut status = 0;
        // SAFETY: the leader is already gone; this must report ECHILD.
        assert_eq!(
            unsafe { libc::waitpid(parent, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        wait_until(
            || match std::fs::read_to_string(format!("/proc/{child}/stat")) {
                Ok(stat) => stat
                    .rsplit_once(") ")
                    .is_some_and(|(_, tail)| tail.starts_with('Z')),
                Err(error) => error.kind() == io::ErrorKind::NotFound,
            },
        );
    }

    #[test]
    fn completion_toasts_cover_every_terminal_status_and_bound_the_tail() {
        for (status, summary) in [
            (Status::Exited(Some(0)), "Tidy done"),
            (Status::Exited(Some(7)), "Tidy failed (exit 7)"),
            (Status::Exited(None), "Tidy failed"),
            (Status::FailedToStart("absent".into()), "Tidy failed"),
            (Status::Failed("wait failed".into()), "Tidy failed"),
        ] {
            let payload = notification("ced", "Tidy", &status, "1\n2\n3\n4\n5\n6\n");
            assert_eq!(payload["summary"], summary);
            assert_eq!(payload["body"], "2\n3\n4\n5\n6");
            assert_eq!(payload["app"], "ced");
            assert_eq!(payload["actions"], serde_json::json!([]));
        }
        assert_eq!(
            notification("ced", "Tidy", &Status::Exited(Some(0)), "")["body"],
            "done"
        );
        assert_eq!(
            notification(
                "ced",
                "Tidy",
                &Status::FailedToStart("permission denied".into()),
                ""
            )["body"],
            "permission denied"
        );
        let payload = notification(
            "ced",
            &"x".repeat(300),
            &Status::Exited(Some(1)),
            &"世".repeat(600),
        );
        assert!(payload["summary"].as_str().unwrap().chars().count() <= 120);
        assert_eq!(payload["body"].as_str().unwrap().chars().count(), 500);
    }

    #[test]
    fn menu_limits_app_rows_and_offers_stop() {
        let catalogue = Catalogue {
            entries: (0..20)
                .map(|n| Entry {
                    id: format!("id-{n}"),
                    title: format!("Title {n}"),
                    path: format!("{n}.mix").into(),
                    tier: if n % 2 == 0 {
                        Tier::BuiltinApp
                    } else {
                        Tier::UserApp
                    },
                    shadows: vec![],
                    targets: None,
                })
                .collect(),
            diagnostics: vec![],
        };
        let (menu, indices) = macro_menu(&catalogue, Some("Active"));
        assert_eq!(menu.entries[0].row().unwrap().label, "Stop \"Active\"");
        assert_eq!(indices.len(), 15);
        assert!(
            menu.entries
                .iter()
                .filter_map(MenuEntry::row)
                .filter(|r| r.id.is_some_and(|id| SLOTS.contains(&id)))
                .all(|r| !r.enabled)
        );
        assert!(
            indices[..10]
                .iter()
                .all(|&i| catalogue.entries[i].tier.is_app())
        );
        assert_eq!(
            menu.entries.last().unwrap().row().unwrap().label,
            "More in Workshop…"
        );
    }

    #[test]
    fn empty_catalogue_explains_itself_with_a_disabled_row() {
        let (menu, indices) = macro_menu(&Catalogue::default(), None);
        assert!(indices.is_empty());
        assert!(matches!(menu.entries[2], MenuEntry::Separator));
        let empty = menu.entries[3].row().unwrap();
        assert_eq!(empty.label, "No macros yet");
        assert!(!empty.enabled);
        assert_eq!(menu.entries.len(), 4);
        assert_eq!(
            menu.entries.last().unwrap().row().unwrap().label,
            "No macros yet"
        );
    }

    #[test]
    fn editor_uses_remaining_height_and_only_reserves_its_output() {
        let mut heights = Vec::new();
        for output in ["", "Hello\n"] {
            let mut state = State::new("ced");
            state.editor_open = true;
            state.document = Some(editor::local::Document::new("print(1)\n", "text").unwrap());
            state.retained = Some(Finished {
                title: "Example".into(),
                output: output.into(),
                status: "done".into(),
            });
            let mut h = egui_kittest::Harness::builder()
                .with_size(egui::vec2(760.0, 560.0))
                .build_ui_state(
                    |ui, state: &mut State| {
                        state.editor_ui(ui, egui::ViewportId::ROOT, egui::ViewportId::ROOT);
                    },
                    state,
                );
            crate::install(&h.ctx, &crate::Theme::embedded());
            h.run_steps(5);
            let editor = h.get_by_role(egui::accesskit::Role::TextInput).rect();
            assert!(editor.height() > 280.0, "{editor:?}");
            if output.is_empty() {
                assert!(editor.bottom() > 540.0, "{editor:?}");
            }
            heights.push(editor.height());
        }
        assert!(heights[0] - heights[1] > 100.0);
    }

    #[test]
    fn macro_editor_snapshot() {
        let mut state = State::new("ced");
        state.editor_open = true;
        state.document = Some(
            editor::local::Document::new("-- title: Hello desktop\nprint(\"Hello\")\n", "text")
                .unwrap(),
        );
        state.retained = Some(Finished {
            title: "Hello desktop".into(),
            output: "Hello\n".into(),
            status: "Hello desktop done".into(),
        });
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(800.0, 700.0))
            .wgpu()
            .build_ui_state(
                |ui, state: &mut State| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        ui.label("Application remains usable");
                    });
                    state.show_editor(ui.ctx());
                },
                state,
            );
        crate::install(&h.ctx, &crate::Theme::embedded());
        h.run_steps(40);
        h.snapshot("macros_editor");
    }

    #[test]
    fn failed_creation_keeps_the_title_field_and_its_text() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new("ced");
        state.title = Some("Tidy imports".into());
        state.open = true;
        let user = temp.path().join("not-a-directory");
        std::fs::write(&user, "obstruction").unwrap();
        state.set_test_roots(Roots {
            user,
            builtin: temp.path().join("builtin"),
            home: temp.path().into(),
            mix: "/opt/mixos/bin/mix".into(),
        });
        state.create_title();
        let result = state
            .creation
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(result.is_err());
        state.created(result, &egui::Context::default());
        assert_eq!(state.title.as_deref(), Some("Tidy imports"));
        assert!(state.open);
        assert!(state.creation_error.is_some());
        assert!(!state.editor_open);
    }

    #[cfg(unix)]
    struct ReadOnlyParent {
        path: std::path::PathBuf,
        permissions: std::fs::Permissions,
    }

    #[cfg(unix)]
    impl ReadOnlyParent {
        fn new(path: &Path) -> Option<Self> {
            use std::os::unix::fs::PermissionsExt;
            // SAFETY: geteuid has no arguments or side effects.
            if unsafe { libc::geteuid() } == 0 {
                eprintln!("skip: root bypasses read-only directory permissions");
                return None;
            }
            let permissions = std::fs::metadata(path).unwrap().permissions();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o500)).unwrap();
            Some(Self {
                path: path.into(),
                permissions,
            })
        }
    }

    #[cfg(unix)]
    impl Drop for ReadOnlyParent {
        fn drop(&mut self) {
            std::fs::set_permissions(&self.path, self.permissions.clone()).unwrap();
        }
    }

    #[cfg(unix)]
    fn permission_harness(state: State) -> egui_kittest::Harness<'static, State> {
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1275.0, 700.0))
            .build_ui_state(|ui, state: &mut State| state.show(ui.ctx()), state);
        crate::install(&h.ctx, &crate::Theme::embedded());
        h.run_steps(5);
        h
    }

    #[cfg(unix)]
    #[test]
    fn read_only_parent_creation_reports_its_path_inline_without_an_editor() {
        let (_temp, roots, _) = runner_fixture();
        let Some(_readonly) = ReadOnlyParent::new(&roots.user) else {
            return;
        };
        // The existing app folder must also be read-only: atomic creates
        // need its permissions even when its ancestor is already present.
        let folder = roots.folder("ced", Tier::UserApp).unwrap();
        let _app_readonly = ReadOnlyParent::new(&folder).unwrap();
        let mut state = State::new("ced");
        state.set_test_roots(roots);
        state.loaded = true;
        state.open = true;
        state.title = Some("Abc".into());
        state.create_title();
        let mut h = permission_harness(state);
        wait_until(|| {
            h.run_steps(1);
            h.state().creation.is_none()
        });
        let error = h.state().creation_error.as_ref().unwrap();
        assert!(error.contains(&folder.display().to_string()));
        assert!(error.to_lowercase().contains("permission denied"));
        let _ = h.get_by_label(error);
        assert_eq!(h.state().title.as_deref(), Some("Abc"));
        assert!(h.state().open);
        assert!(!h.state().editor_open);
        assert!(h.state().document.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn read_only_parent_copy_reports_its_path_inline_without_an_editor() {
        let (_temp, roots, _) = runner_fixture();
        let folder = roots.folder("ced", Tier::UserApp).unwrap();
        let Some(_readonly) = ReadOnlyParent::new(&folder) else {
            return;
        };
        let builtin = roots.builtin.join("ced");
        std::fs::create_dir_all(&builtin).unwrap();
        let source = builtin.join("builtin.mix");
        std::fs::write(&source, "-- title: Built-in\nprint(1)\n").unwrap();
        let entry = Entry {
            id: "builtin".into(),
            title: "Built-in".into(),
            path: source,
            tier: Tier::BuiltinApp,
            shadows: vec![],
            targets: None,
        };
        let destination = folder.join("builtin.mix");
        let mut state = State::new("ced");
        state.set_test_roots(roots);
        state.loaded = true;
        state.edit(entry);
        let mut h = permission_harness(state);
        wait_until(|| {
            h.run_steps(1);
            h.state().pending.is_none()
        });
        let error = h.state().error.as_ref().unwrap();
        assert!(error.contains(&destination.display().to_string()));
        assert!(error.to_lowercase().contains("permission denied"));
        let _ = h.get_by_label(error);
        assert!(h.state().open);
        assert!(!h.state().editor_open);
        assert!(h.state().document.is_none());
        assert!(!destination.exists());
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_or_missing_user_files_never_open_an_editor() {
        let (_temp, roots, entry) = runner_fixture();
        let folder = roots.folder("ced", Tier::UserApp).unwrap();
        let Some(_readonly) = ReadOnlyParent::new(&folder) else {
            return;
        };
        for exists in [true, false] {
            let mut entry = entry.clone();
            if !exists {
                entry.path = folder.join("missing.mix");
            }
            let path = entry.path.clone();
            let mut state = State::new("ced");
            state.set_test_roots(roots.clone());
            state.loaded = true;
            state.edit(entry);
            let mut h = permission_harness(state);
            wait_until(|| {
                h.run_steps(1);
                h.state().pending.is_none()
            });
            let error = h.state().error.as_ref().unwrap();
            assert!(error.contains(&path.display().to_string()));
            if exists {
                assert!(error.to_lowercase().contains("permission denied"));
            }
            let _ = h.get_by_label(error);
            assert!(!h.state().editor_open);
            assert!(h.state().document.is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn macro_editor_refuses_symlinks_without_touching_the_target() {
        let (_temp, roots, entry) = runner_fixture();
        let link = entry.path.with_file_name("linked.mix");
        let before = std::fs::read(&entry.path).unwrap();
        std::os::unix::fs::symlink(&entry.path, &link).unwrap();
        assert!(open_macro(&roots, &link).is_err());
        assert!(roots.write_path(&link, b"changed").is_err());
        assert_eq!(std::fs::read(&entry.path).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn read_only_macro_file_is_refused_inline_in_a_writable_directory() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, roots, entry) = runner_fixture();
        std::fs::set_permissions(&entry.path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let before = std::fs::read(&entry.path).unwrap();
        let mut state = State::new("ced");
        state.set_test_roots(roots);
        state.loaded = true;
        state.edit(entry.clone());
        let mut h = permission_harness(state);
        wait_until(|| {
            h.run_steps(1);
            h.state().pending.is_none()
        });
        let error = h.state().error.as_ref().unwrap();
        assert!(error.contains(&entry.path.display().to_string()));
        assert!(error.contains("is read-only"));
        let _ = h.get_by_label(error);
        assert!(h.state().open);
        assert!(!h.state().editor_open);
        assert!(h.state().document.is_none());
        assert_eq!(std::fs::read(&entry.path).unwrap(), before);
        assert_eq!(
            std::fs::metadata(&entry.path).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_permission_errors_name_the_path_and_keep_dirty_edits() {
        let (_temp, roots, entry) = runner_fixture();
        let mut document = open_macro(&roots, &entry.path).unwrap();
        document
            .command(
                editor::EditCommand::Insert("-- edit\n".into()),
                &editor::View::default(),
            )
            .unwrap();
        let folder = roots.folder("ced", Tier::UserApp).unwrap();
        let Some(_readonly) = ReadOnlyParent::new(&folder) else {
            return;
        };
        let mut state = State::new("ced");
        state.set_test_roots(roots);
        state.loaded = true;
        state.document = Some(document);
        state.editor_open = true;
        state.save_editor(true, false);
        let mut h = permission_harness(state);
        wait_until(|| {
            h.run_steps(1);
            h.state().saving.is_none()
        });
        let error = h.state().error.as_ref().unwrap();
        assert!(error.contains(&entry.path.display().to_string()));
        assert!(error.to_lowercase().contains("permission denied"));
        let _ = h.get_by_label(error);
        assert!(h.state().editor_open);
        assert!(h.state().document.as_ref().unwrap().is_dirty());
        assert!(
            h.state()
                .document
                .as_ref()
                .unwrap()
                .contents()
                .contains("-- edit")
        );
    }

    #[cfg(unix)]
    #[test]
    fn saves_during_discovery_keep_the_editor_visible_and_close_saves_later_edits() {
        let (_temp, roots, entry) = runner_fixture();
        let mut state = State::new("ced");
        state.set_test_roots(roots);
        state.editor_open = true;
        state.document = Some(editor::local::Document::open(&entry.path).unwrap());
        let (hold, pending) = mpsc::channel::<Completion>();
        state.pending = Some(pending); // a catalogue scan has not completed.
        state
            .document
            .as_mut()
            .unwrap()
            .command(
                editor::EditCommand::Insert("-- first\n".into()),
                &editor::View::default(),
            )
            .unwrap();
        state.save_editor(true, false);
        assert!(state.pending.is_some());
        assert!(state.document.is_some());
        assert!(state.saving.is_some(), "saving must not wait for discovery");
        state
            .document
            .as_mut()
            .unwrap()
            .command(
                editor::EditCommand::Insert("-- later\n".into()),
                &editor::View::default(),
            )
            .unwrap();
        state.save_editor(true, false);
        assert_eq!(
            state.save_request,
            Some(false),
            "a second Ctrl+S stays queued during the first write"
        );
        let result = state
            .saving
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        let ctx = egui::Context::default();
        state.saved(result, &ctx);
        assert!(
            state.document.as_ref().unwrap().is_dirty(),
            "newer edits must stay dirty"
        );
        state.save_editor(false, false);
        assert!(
            state.saving.is_some(),
            "the queued save must write the newer snapshot"
        );
        state.close_editor();
        assert!(state.editor_open, "close waits for the save receipt");
        let result = state
            .saving
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        state.saved(result, &ctx);
        assert!(!state.editor_open);
        let doc = state.document.as_ref().unwrap();
        assert!(!doc.is_dirty());
        assert_eq!(
            std::fs::read_to_string(&entry.path).unwrap(),
            doc.contents()
        );
        drop(hold);
    }

    #[test]
    fn failed_save_on_close_keeps_dirty_text_visible() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("gone");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("macro.mix");
        std::fs::write(&path, "original").unwrap();
        let mut doc = editor::local::Document::open(&path).unwrap();
        doc.command(
            editor::EditCommand::Insert("new ".into()),
            &editor::View::default(),
        )
        .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        let mut state = State::new("ced");
        state.document = Some(doc);
        state.editor_open = true;
        state.close_editor();
        let result = state
            .saving
            .take()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(result.is_err());
        state.saved(result, &egui::Context::default());
        assert!(state.editor_open);
        assert!(state.document.as_ref().unwrap().is_dirty());
        assert_eq!(state.document.as_ref().unwrap().contents(), "new original");
        assert!(state.error.is_some());
    }

    #[test]
    fn idle_macro_state_does_not_request_ten_hz_repaints() {
        let mut state = State::new("ced");
        state.loaded = true;
        assert!(!state.needs_repaint());
        let ctx = egui::Context::default();
        let mut delay = Duration::ZERO;
        for frame in 0..40 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(frame as f64),
                    ..Default::default()
                },
                |_| state.show(&ctx),
            );
            delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            output.textures_delta.clear();
        }
        assert!(
            delay > Duration::from_millis(100),
            "idle delay was {delay:?}"
        );
    }

    #[derive(Default)]
    struct AppMenu {
        open: bool,
        modal: bool,
        queued_toggle: bool,
        toggles: usize,
        clicks: usize,
        stolen_keys: usize,
    }

    fn app_harness() -> egui_kittest::Harness<'static, AppMenu> {
        app_harness_at(false, egui::vec2(800.0, 700.0), 1.0)
    }

    fn app_harness_at(
        render: bool,
        size: egui::Vec2,
        scale: f32,
    ) -> egui_kittest::Harness<'static, AppMenu> {
        let temp = tempfile::tempdir().unwrap();
        let builder = egui_kittest::Harness::builder()
            .with_size(size)
            .with_pixels_per_point(scale);
        let builder = if render { builder.wgpu() } else { builder };
        let mut h = builder.build_ui_state(
            move |ui, app: &mut AppMenu| {
                let runtime = window_state(ui.ctx(), "ced", true).unwrap();
                {
                    let mut state = runtime.lock().unwrap();
                    state.loaded = true;
                    state.set_test_roots(Roots {
                        user: temp.path().join("user"),
                        builtin: temp.path().join("builtin"),
                        home: temp.path().into(),
                        mix: temp.path().join("unused"),
                    });
                }
                let mut registry = crate::command::Registry::new();
                registry.add(command(
                    |s: &AppMenu| s.open || !s.modal,
                    |s: &mut AppMenu| {
                        s.open = !s.open;
                        s.toggles += 1;
                    },
                ));
                let strings = crate::Strings::new("toolkit-macros = Macros\nhelp = Help\n");
                let fired =
                    crate::titlebar::show_bar(ui, "Test", None, &registry, app, &strings, &[]);
                egui::CentralPanel::default().show(ui, |ui| {
                    if ui.button("Underlying control").clicked() {
                        app.clicks += 1;
                    }
                    // Deliberately greedy app content: menu keys must already
                    // have been taken by the title bar's pre-content pass.
                    for key in [egui::Key::Enter, egui::Key::Escape] {
                        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                            app.stolen_keys += 1;
                        }
                    }
                });
                let close = show_app(ui.ctx(), "ced", app.open);
                for (id, origin) in fired {
                    registry.execute(id, app, origin).unwrap();
                }
                if std::mem::take(&mut app.queued_toggle) {
                    registry
                        .execute("help.macros", app, crate::command::Origin::Ui)
                        .unwrap();
                }
                if close {
                    app.open = false;
                }
            },
            AppMenu::default(),
        );
        crate::install(&h.ctx, &crate::Theme::embedded());
        h.run_steps(5);
        h
    }

    fn runtime(h: &egui_kittest::Harness<'_, AppMenu>) -> WindowState {
        let id = crate::window_id(egui::ViewportId::ROOT, ("shared-macros", "ced"));
        h.ctx.data(|d| d.get_temp(id)).unwrap()
    }

    #[test]
    fn macro_empty_menu_right_edge_snapshot() {
        let mut h = app_harness_at(true, egui::vec2(1275.0, 700.0), 2.5);
        h.get_by_label("Macros").click();
        h.run_steps(40);
        let row = h.get_by_label("New macro…").rect();
        let anchor = h
            .ctx
            .data(|d| {
                d.get_temp::<egui::Rect>(crate::window_id(egui::ViewportId::ROOT, "macro-anchor"))
            })
            .unwrap();
        let chrome = crate::chrome::Chrome::of(&h.ctx);
        let margin = egui::Frame::menu(&h.ctx.global_style()).total_margin();
        assert!(row.width() >= chrome.metrics.menu_min_width);
        assert!((row.right() + margin.right - anchor.right()).abs() < 1.0);
        assert!(h.ctx.content_rect().contains_rect(row));
        let _ = h.get_by_label("No macros yet");
        h.snapshot("macros_menu_empty_right_edge");
    }

    fn title_field_harness(
        render: bool,
        wide: bool,
    ) -> (egui_kittest::Harness<'static, AppMenu>, egui::Rect) {
        let mut h = app_harness_at(render, egui::vec2(1275.0, 700.0), 2.5);
        if wide {
            let runtime = runtime(&h);
            let mut state = runtime.lock().unwrap();
            state.catalogue.entries.push(Entry {
                id: "wide".into(),
                title: "A much longer macro title that determines the menu width".into(),
                path: "wide.mix".into(),
                tier: Tier::UserApp,
                shadows: vec![],
                targets: None,
            });
            state.engine.replace_catalogue(state.catalogue.clone());
        }
        h.get_by_label("Macros").click();
        h.run_steps(40);
        let menu_row = h.get_by_label("New macro…").rect();
        h.get_by_label("New macro…").click();
        h.run_steps(40);
        (h, menu_row)
    }

    #[test]
    fn new_title_uses_the_menu_row_width_height_and_right_edge_placement_at_scale() {
        for wide in [false, true] {
            let (h, menu_row) = title_field_harness(false, wide);
            let field = h.get_by_role(egui::accesskit::Role::TextInput).rect();
            let chrome = crate::chrome::Chrome::of(&h.ctx);
            let margin = egui::Frame::menu(&h.ctx.global_style()).total_margin();
            assert!(field.width() >= chrome.metrics.menu_min_width - margin.sum().x);
            assert!(field.height() >= chrome.metrics.menu_row_height);
            assert!(field.height() >= h.ctx.global_style().spacing.interact_size.y);
            assert!((field.width() - menu_row.width()).abs() < 1.0);
            assert!((field.left() - menu_row.left()).abs() < 1.0);
            assert!((field.right() - menu_row.right()).abs() < 1.0);
            assert!(h.ctx.content_rect().contains_rect(field));
            let anchor = h
                .ctx
                .data(|d| {
                    d.get_temp::<egui::Rect>(crate::window_id(
                        egui::ViewportId::ROOT,
                        "macro-anchor",
                    ))
                })
                .unwrap();
            assert!(field.top() >= anchor.bottom());
            assert!((field.right() + margin.right - anchor.right()).abs() < 1.0);
            assert!(
                h.ctx.memory(
                    |m| m.has_focus(crate::window_id(egui::ViewportId::ROOT, "macro-title"))
                )
            );
        }
    }

    #[test]
    fn title_creation_errors_wrap_without_widening_the_frame() {
        let (mut h, _) = title_field_harness(false, false);
        let original = h.get_by_role(egui::accesskit::Role::TextInput).rect();
        let error = format!(
            "Can't create /{}: permission denied",
            "a-long-folder-name/".repeat(8)
        );
        runtime(&h).lock().unwrap().creation_error = Some(error.clone());
        h.run_steps(40);
        let field = h.get_by_role(egui::accesskit::Role::TextInput).rect();
        let message = h.get_by_label(&error).rect();
        assert!((field.width() - original.width()).abs() < 1.0);
        assert!((field.left() - original.left()).abs() < 1.0);
        assert!(message.width() <= field.width() + 1.0);
        assert!(message.height() > h.ctx.global_style().spacing.interact_size.y);
        assert!(h.ctx.content_rect().contains_rect(message));
        assert!(h.ctx.content_rect().contains_rect(field));
    }

    #[test]
    fn macro_title_field_snapshot() {
        let (mut h, _) = title_field_harness(true, false);
        h.event(egui::Event::Text("Tidy imports".into()));
        h.run_steps(40);
        assert_eq!(
            runtime(&h).lock().unwrap().title.as_deref(),
            Some("Tidy imports")
        );
        h.snapshot("macros_title_field");
    }

    #[test]
    fn show_app_icon_escape_and_same_frame_toggle_close_stay_in_sync() {
        let mut h = app_harness();
        h.get_by_label("Macros").click();
        h.run_steps(5);
        assert!(h.state().open);
        h.get_by_label("Macros").click();
        h.run_steps(5);
        assert!(!h.state().open);
        h.get_by_label("Macros").click();
        h.run_steps(5);
        h.state_mut().modal = true;
        h.state_mut().queued_toggle = true;
        h.key_press(egui::Key::Escape);
        h.run_steps(5);
        assert!(!h.state().open);
        assert!(!menu::is_open(&h.ctx));
        assert_eq!(h.state().stolen_keys, 0);
        h.state_mut().modal = false;
        h.get_by_label("Macros").click();
        h.run_steps(5);
        assert!(
            h.state().open,
            "an old dismissal must not close a later opening"
        );
    }

    #[test]
    fn show_app_new_title_takes_enter_and_escape_before_app_content() {
        let mut h = app_harness();
        h.get_by_label("Macros").click();
        h.run_steps(5);
        h.get_by_label("New macro…").click();
        h.run_steps(5);
        assert_eq!(runtime(&h).lock().unwrap().title.as_deref(), Some(""));
        assert!(h.state().open);
        assert!(
            h.ctx
                .memory(|m| m.has_focus(crate::window_id(egui::ViewportId::ROOT, "macro-title")))
        );
        h.key_press(egui::Key::Escape);
        h.run_steps(5);
        assert!(!h.state().open);
        assert!(
            !h.ctx
                .memory(|m| m.has_focus(crate::window_id(egui::ViewportId::ROOT, "macro-title")))
        );
        assert_eq!(h.state().stolen_keys, 0);
        h.get_by_label("Macros").click();
        h.run_steps(5);
        h.get_by_label("New macro…").click();
        h.run_steps(5);
        h.event(egui::Event::Text("Tidy imports".into()));
        h.key_press(egui::Key::Enter);
        h.run_steps(5);
        wait_until(|| {
            h.run_steps(1);
            runtime(&h).lock().unwrap().creation.is_none()
        });
        assert_eq!(h.state().stolen_keys, 0);
        assert!(!h.state().open);
        assert!(runtime(&h).lock().unwrap().document.is_some());
    }

    #[test]
    fn show_app_failed_creation_reports_inline_and_restores_title_focus() {
        let mut h = app_harness();
        let roots = runtime(&h).lock().unwrap().engine.roots().unwrap();
        std::fs::write(&roots.user, "obstruction").unwrap();
        h.get_by_label("Macros").click();
        h.run_steps(5);
        h.get_by_label("New macro…").click();
        h.run_steps(5);
        h.event(egui::Event::Text("Tidy imports".into()));
        h.key_press(egui::Key::Enter);
        h.run_steps(5);
        wait_until(|| {
            h.run_steps(1);
            runtime(&h).lock().unwrap().creation_error.is_some()
        });
        h.run_steps(5);
        let state = runtime(&h);
        let state = state.lock().unwrap();
        assert!(state.creation.is_none());
        assert_eq!(state.title.as_deref(), Some("Tidy imports"));
        assert!(h.state().open);
        assert!(!state.editor_open);
        let _ = h.get_by_label(state.creation_error.as_ref().unwrap());
        assert!(
            h.ctx
                .memory(|m| m.has_focus(crate::window_id(egui::ViewportId::ROOT, "macro-title")))
        );
        assert_eq!(h.state().stolen_keys, 0);
    }

    #[test]
    fn refreshed_catalogue_discards_a_pick_from_the_previous_menu() {
        let mut h = app_harness();
        let entry = |id: &str| Entry {
            id: id.into(),
            title: id.into(),
            tier: Tier::UserApp,
            path: format!("{id}.mix").into(),
            shadows: vec![],
            targets: None,
        };
        runtime(&h).lock().unwrap().catalogue.entries = vec![entry("alpha")];
        {
            let state = runtime(&h);
            let state = state.lock().unwrap();
            state.engine.replace_catalogue(state.catalogue.clone());
        }
        h.get_by_label("Macros").click();
        h.run_steps(5);
        h.event(egui::Event::Text("a".into()));
        h.run_steps(1);
        let (tx, rx) = mpsc::channel();
        runtime(&h)
            .lock()
            .unwrap()
            .engine
            .replace_catalogue(Catalogue {
                entries: vec![entry("before"), entry("alpha")],
                diagnostics: vec![],
            });
        assert!(
            tx.send(Ok((
                Catalogue {
                    entries: vec![entry("before"), entry("alpha")],
                    diagnostics: vec![]
                },
                None,
                None
            )))
            .is_ok()
        );
        runtime(&h).lock().unwrap().pending = Some(rx);
        h.key_press(egui::Key::Enter);
        h.run_steps(5);
        let state = runtime(&h);
        let state = state.lock().unwrap();
        assert_eq!(state.engine.runner_status(), Status::Idle);
        assert!(state.document.is_none());
        assert!(!h.state().open);
    }

    #[test]
    fn outside_presses_dismiss_dropdown_and_title_and_reach_the_underlying_control() {
        for title in [false, true] {
            let mut h = app_harness();
            h.get_by_label("Macros").click();
            h.run_steps(5);
            if title {
                h.get_by_label("New macro…").click();
                h.run_steps(5);
            }
            let pos = h.get_by_label("Underlying control").rect().center();
            h.event(egui::Event::PointerMoved(pos));
            h.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            h.run_steps(1);
            assert!(!h.state().open);
            assert!(
                !h.ctx.memory(
                    |m| m.has_focus(crate::window_id(egui::ViewportId::ROOT, "macro-title"))
                )
            );
            h.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
            h.run_steps(5);
            assert_eq!(h.state().clicks, 1);
        }
    }

    #[test]
    fn app_exit_releases_state_even_with_a_retained_repaint_context() {
        let ctx = egui::Context::default();
        let mut weak = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {
            let state = window_state(&ctx, "ced", true).unwrap();
            weak = Some(Arc::downgrade(&state));
        });
        output.textures_delta.clear();
        let repaint_context = ctx.clone();
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        let mut output = ctx.run_ui(input, |_| {
            // The app may cancel this request for its own unsaved work.
            assert!(window_state(&ctx, "ced", false).is_some());
        });
        output.textures_delta.clear();
        assert!(weak.as_ref().unwrap().upgrade().is_some());
        close_app(&ctx, "ced");
        assert!(weak.unwrap().upgrade().is_none());
        drop(repaint_context);
    }

    #[test]
    fn root_state_survives_frames_with_no_raw_viewports() {
        let ctx = egui::Context::default();
        let mut first = None;
        for frame in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |_| {
                // Exercise the adapter with the host's viewport map absent.
                // Restore it before egui's own end-pass bookkeeping.
                let viewports = ctx.input_mut(|i| std::mem::take(&mut i.raw.viewports));
                assert!(ctx.input(|i| i.raw.viewports.is_empty()));
                let state = window_state(&ctx, "ced", frame == 0).unwrap();
                if let Some(first) = &first {
                    assert!(Arc::ptr_eq(first, &state));
                } else {
                    first = Some(state);
                }
                ctx.input_mut(|i| i.raw.viewports = viewports);
            });
            out.textures_delta.clear();
        }
    }

    #[test]
    fn idle_second_window_does_not_cancel_the_first_windows_editor() {
        let ctx = egui::Context::default();
        crate::drive::install(&ctx, "dopus");
        ctx.set_embed_viewports(false);
        let mut a = State::new("dopus");
        let mut b = State::new("dopus");
        a.editor_open = true;
        a.document = Some(editor::local::Document::new("print(1)\n", "text").unwrap());
        let editor = egui::ViewportId::from_hash_of(("macro-editor", a.window));
        let mut frame = |child: bool| {
            let mut input = egui::RawInput {
                viewport_id: if child {
                    editor
                } else {
                    egui::ViewportId::ROOT
                },
                ..Default::default()
            };
            input.viewports.insert(editor, Default::default());
            let mut output = ctx.run_ui(input, |ui| {
                if child {
                    a.editor_ui(ui, egui::ViewportId::ROOT, editor);
                } else {
                    ui.ctx().show_viewport_deferred(
                        editor,
                        egui::ViewportBuilder::default(),
                        |_, _| {},
                    );
                    b.show_editor(ui.ctx());
                }
            });
            output.textures_delta.clear();
        };
        for _ in 0..3 {
            frame(false);
            frame(true);
        }
        for id in 1..=3 {
            assert!(
                crate::drive::request(
                    &ctx,
                    id,
                    "ui.tree",
                    &serde_json::json!({"viewport":"macro-editor"})
                )
                .is_none()
            );
            frame(false);
            frame(true);
            let answers = crate::drive::logic(&ctx);
            assert_eq!(answers.len(), 1);
            assert_eq!(answers[0].rc, 0);
            assert!(answers[0].body.to_string().contains("Save"));
        }
    }

    #[test]
    fn viewports_have_separate_state_and_closed_viewports_are_released() {
        let ctx = egui::Context::default();
        let second = egui::ViewportId::from_hash_of("second");
        let mut states = Vec::new();
        let mut ids = Vec::new();
        for viewport in [egui::ViewportId::ROOT, second] {
            let mut input = egui::RawInput {
                viewport_id: viewport,
                ..Default::default()
            };
            input.viewports.insert(second, Default::default());
            let mut out = ctx.run_ui(input, |_| {
                states.push(window_state(&ctx, "ced", true).unwrap());
                ids.push(crate::window_id(viewport, "macro-editor"));
            });
            out.textures_delta.clear();
        }
        assert!(!Arc::ptr_eq(&states[0], &states[1]));
        assert_ne!(ids[0], ids[1]);
        assert_ne!(
            states[0].lock().unwrap().window,
            states[1].lock().unwrap().window
        );
        let mut input = egui::RawInput::default();
        input.viewports.insert(second, Default::default());
        let mut out = ctx.run_ui(input, |_| {
            close_app(&ctx, "ced");
            assert!(
                ctx.data(|d| d.get_temp::<WindowState>(crate::window_id(
                    egui::ViewportId::ROOT,
                    ("shared-macros", "ced")
                )))
                .is_none()
            );
            let remaining = ctx
                .data(|d| {
                    d.get_temp::<WindowState>(crate::window_id(second, ("shared-macros", "ced")))
                })
                .unwrap();
            assert!(Arc::ptr_eq(&remaining, &states[1]));
        });
        out.textures_delta.clear();
        let weak = Arc::downgrade(&states[1]);
        states.pop();
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {
            let _ = window_state(&ctx, "ced", false);
        });
        out.textures_delta.clear();
        assert!(weak.upgrade().is_none());
    }
}
