// SPDX-License-Identifier: MIT OR Apache-2.0
//! Per-window macro menu, editor and background runtime.
mod notifications;
use crate::menu::{self, Entry as MenuEntry, Menu, Row};
use crate::strings::{own, own_with};
use ::macros::{Catalogue, Entry, Roots, Runner, Status, Tier};
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
type Creation = Result<(Catalogue, editor::local::Document), String>;
type SaveCompletion = Result<(editor::local::Saved, Option<Catalogue>, Option<Entry>), String>;
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

/// Macro execution and catalogue state, kept together for the shared Session
/// integration. The window's menu, editor and notification state stay outside.
pub struct Engine {
    pub catalogue: Catalogue,
    pub runner: Runner,
    roots: Option<Roots>,
    running: Option<Entry>,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            catalogue: Catalogue::default(),
            runner: Runner::default(),
            roots: Roots::resolve(),
            running: None,
        }
    }
}

/// Runtime resources stay outside app engines and serialised app state.
pub struct State {
    pub open: bool,
    pub engine: Engine,
    app: String,
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
    editor_focus: Option<egui::Id>,
    retained: Option<Finished>,
}

impl State {
    pub fn new(app: &str) -> Self {
        let (notify_tx, notifications) = mpsc::channel();
        Self {
            open: false,
            engine: Engine::default(),
            app: app.into(),
            pending: None,
            notifications,
            notify_tx,
            queued: VecDeque::new(),
            lifetime: Arc::new(()),
            window: NEXT_WINDOW.fetch_add(1, Ordering::Relaxed),
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
        let Some(roots) = self.engine.roots.clone() else {
            self.error = Some(own("macros-no-home"));
            self.open = true;
            return;
        };
        let app = self.app.clone();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.error = None;
        std::thread::spawn(move || {
            let result = task(&roots, &app)
                .and_then(|(doc, run)| Ok((Catalogue::scan(&roots, &app)?, doc, run)))
                .map_err(|e| (e.to_string(), None));
            let _ = tx.send(result);
        });
    }

    fn run(&mut self, entry: Entry, ctx: &egui::Context) {
        if self.engine.runner.status() == Status::Running {
            self.run_message = Some(own("macros-already-running"));
            self.editor_open = true;
            return;
        }
        // Completion can race a menu click between frames. Retain and report
        // the previous terminal run before Runner resets its output buffer.
        self.finish_run(ctx);
        self.run_message = None;
        if let Some(roots) = &self.engine.roots {
            // Thread-spawn failure sets a terminal status too; polling it
            // follows the same notification path as a child failing to start.
            let _ = self.engine.runner.start(roots, &self.app, &entry);
            self.engine.running = Some(entry);
            self.retained = None;
        } else {
            self.error = Some(own("macros-no-home"));
            self.editor_open = true;
        }
    }

    fn finish_run(&mut self, ctx: &egui::Context) {
        let status = self.engine.runner.status();
        if status == Status::Running {
            return;
        }
        if let Some(entry) = self.engine.running.take() {
            self.run_message = None;
            let output = self.engine.runner.output().text;
            let payload = notification(&self.app, &entry.title, &status, &output);
            let finished = Finished {
                title: entry.title,
                output,
                status: payload["summary"].as_str().unwrap_or_default().into(),
            };
            self.retained = Some(finished.clone());
            notifications::post(
                &self.app,
                self.window,
                payload,
                finished,
                self.notify_tx.clone(),
                &self.lifetime,
                ctx,
            );
        }
    }

    fn create_title(&mut self) {
        if self.creation.is_some() {
            return;
        }
        let Some(title) = self.title.clone() else {
            return;
        };
        let Some(roots) = self.engine.roots.clone() else {
            self.creation_error = Some(own("macros-no-home"));
            return;
        };
        self.creation_error = None;
        let app = self.app.clone();
        let (tx, rx) = mpsc::channel();
        self.creation = Some(rx);
        std::thread::spawn(move || {
            let result = (|| {
                let folder = roots.folder(&app, Tier::UserApp)?;
                let path = roots
                    .create_from_title(&app, &title)
                    .map_err(|e| path_error("macros-create-error", &folder, e))?;
                let doc = open_macro(&path)?;
                Ok((Catalogue::scan(&roots, &app)?, doc))
            })()
            .map_err(|e: io::Error| e.to_string());
            let _ = tx.send(result);
        });
    }

    fn created(&mut self, result: Creation, ctx: &egui::Context) {
        let viewport = ctx.viewport_id();
        self.creation = None;
        match result {
            Ok((catalogue, doc)) => {
                self.replace_catalogue(catalogue, ctx);
                self.editor_entry = self
                    .engine
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
            Ok((Some(open_macro(&path)?), None))
        });
    }

    fn can_replace_document(&mut self) -> bool {
        if self.saving.is_some()
            || self.close_after_save
            || self.document.as_ref().is_some_and(|doc| doc.is_dirty())
        {
            self.error = Some(own("macros-save-first"));
            self.editor_open = true;
            false
        } else {
            true
        }
    }

    fn folder(&mut self) {
        self.work(|roots, app| {
            launch(&roots.ensure_user_folder(app, Tier::UserApp)?, true)?;
            Ok((None, None))
        });
    }

    pub fn show(&mut self, ctx: &egui::Context) {
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
                                .engine
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
                                .engine
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
        self.finish_run(ctx);
        while let Ok(finished) = self.notifications.try_recv() {
            self.retained = Some(finished);
            self.editor_open = true;
        }
        ctx.data_mut(|d| {
            d.insert_temp(
                crate::window_id(viewport, "macro-busy"),
                self.engine.runner.status() == Status::Running,
            )
        });
        if self.open && !self.loaded && self.pending.is_none() {
            self.reload();
        }
        let anchor = ctx
            .data(|d| d.get_temp::<egui::Rect>(crate::window_id(viewport, "macro-anchor")))
            .unwrap_or_else(|| {
                egui::Rect::from_min_size(ctx.content_rect().left_top(), egui::Vec2::ZERO)
            });
        let mut selected = None;
        egui::Area::new(crate::window_id(viewport, "macro-menu-owner"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor.left_bottom())
            .show(ctx, |ui| {
                if let Some(title) = &mut self.title {
                    if self.open {
                        menu::anchored_open(ctx, true);
                        let shown = egui::Frame::menu(ui.style()).show(ui, |ui| {
                            let (enter, escape) = menu::title_keys(ctx);
                            let field = ui.add_enabled(
                                self.creation.is_none(),
                                egui::TextEdit::singleline(title)
                                    .id(crate::window_id(viewport, "macro-title"))
                                    .hint_text(own("macros-title")),
                            );
                            if let Some(error) = &self.creation_error {
                                ui.label(error);
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
                                        crate::window_id(viewport, "macro-title-focused"),
                                        true,
                                    )
                                });
                            }
                            (field, enter, escape)
                        });
                        menu::title_bounds(ctx, shown.response.rect);
                        let (field, enter, escape) = shown.inner;
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
                    let (mut model, indices) = macro_menu(
                        &self.engine.catalogue,
                        self.engine.running.as_ref().map(|e| e.title.as_str()),
                    );
                    if let Some(error) = &self.error {
                        model.entries.push(MenuEntry::Separator);
                        model.entries.push(MenuEntry::Row(Row::command(
                            "macro.error",
                            error,
                            None,
                            false,
                        )));
                    }
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
                            let entry = self.engine.catalogue.entries[indices[slot]].clone();
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
                "macro.stop" => self.engine.runner.stop(),
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
        self.engine.runner.status() == Status::Running
            || self.pending.is_some()
            || self.creation.is_some()
            || self.saving.is_some()
            || self.save_request.is_some()
    }

    fn show_editor(&mut self, ctx: &egui::Context) {
        let viewport = ctx.viewport_id();
        if !self.editor_open {
            return;
        }
        let mut open = true;
        let mut save = false;
        let mut run = false;
        let mut ced = false;
        let chrome = crate::chrome::Chrome::of(ctx);
        let row_height = chrome.metrics.menu_row_height;
        let output_height = row_height * 4.0;
        egui::Window::new(own("macros-editor"))
            .id(crate::window_id(viewport, "macro-editor"))
            .open(&mut open)
            .default_size(egui::vec2(
                chrome.style.dialog_max_width as f32,
                row_height * 18.0,
            ))
            .show(ctx, |ui| {
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
                    .data_mut(|d| {
                        d.remove_temp::<bool>(crate::window_id(viewport, "macro-save-run"))
                    })
                    .unwrap_or(false);
                let editor_enabled = !menu::is_open(ctx);
                if let Some(doc) = &mut self.document {
                    ui.horizontal(|ui| {
                        ui.label(
                            doc.path()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default(),
                        );
                        ced = ui.link(own("macros-open-ced")).clicked();
                    });
                    let palette = ctx
                        .data(|d| {
                            d.get_temp::<editor::Palette>(egui::Id::new("toolkit.editor-palette"))
                        })
                        .unwrap_or_else(|| editor::Palette::from_theme(&crate::Theme::embedded()));
                    let height = (ui.available_height() - output_height - row_height * 2.0)
                        .max(row_height * 6.0);
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
                if let Some(error) = &self.error {
                    ui.label(error);
                }
                ui.separator();
                if let Some(message) = &self.run_message {
                    ui.label(message);
                }
                let output = self.displayed_output();
                if self.engine.runner.status() != Status::Running
                    && let Some(last) = &self.retained
                {
                    ui.label(&last.status).on_hover_text(&last.title);
                }
                if let Some(running) = &self.engine.running {
                    ui.label(own_with(
                        "macros-running-title",
                        &[("title", &running.title)],
                    ));
                }
                egui::ScrollArea::both()
                    .max_height(output_height)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(output).monospace())
                                .selectable(true),
                        );
                    });
            });
        if !open {
            self.close_editor();
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
    }

    fn save_editor(&mut self, save: bool, mut run: bool) {
        let requested = run;
        if run && self.engine.runner.status() == Status::Running {
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
                    self.editor_open = true;
                    self.close_after_save = false;
                    return;
                }
            };
            let entry = if run { self.editor_entry.clone() } else { None };
            let roots = self.engine.roots.clone();
            let app = self.app.clone();
            let (tx, rx) = mpsc::channel();
            self.saving = Some(rx);
            std::thread::spawn(move || {
                let completion = snapshot
                    .write()
                    .map(|saved| {
                        let catalogue = roots.and_then(|roots| Catalogue::scan(&roots, &app).ok());
                        (saved, catalogue, entry)
                    })
                    .map_err(|error| path_error("macros-save-error", &path, error).to_string());
                let _ = tx.send(completion);
            });
        }
    }

    fn saved(&mut self, result: SaveCompletion, ctx: &egui::Context) {
        self.saving = None;
        match result {
            Ok((saved, catalogue, run)) => {
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
                self.editor_open = true;
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
            self.editor_open = true;
            if self.saving.is_none() && self.save_request.is_none() {
                self.save_editor(true, false);
            }
        } else {
            self.editor_open = false;
        }
    }

    fn displayed_output(&self) -> String {
        if self.engine.runner.status() == Status::Running {
            self.engine.runner.output().text
        } else {
            self.retained
                .as_ref()
                .map(|r| r.output.clone())
                .unwrap_or_default()
        }
    }

    fn replace_catalogue(&mut self, catalogue: Catalogue, ctx: &egui::Context) {
        // Cached keyboard picks refer to the old row-to-file mapping. Closing
        // invalidates both the highlight and any already queued activation.
        if !self.engine.catalogue.entries.is_empty()
            && self.engine.catalogue.entries != catalogue.entries
        {
            menu::dismiss_anchored(ctx);
            if self.title.is_none() {
                self.open = false;
            }
        }
        self.engine.catalogue = catalogue;
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.engine.runner.stop();
        notifications::forget(self.window);
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
    for app in [true, false] {
        entries.push(MenuEntry::Separator);
        for (index, entry) in catalogue
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.tier.is_app() == app)
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
        result.clone()
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
        "app": app, "actions": [{"id": "output", "label": own("macros-show-output")}]
        , "timeout_ms": notifications::RETENTION.as_millis() as u64
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
fn open_macro(path: &Path) -> io::Result<editor::local::Document> {
    let document = editor::local::Document::open(path)
        .map_err(|e| path_error("macros-open-error", path, e))?;
    let target =
        editor::local::save_target(path).map_err(|e| path_error("macros-save-error", path, e))?;
    let metadata =
        std::fs::metadata(target.path()).map_err(|e| path_error("macros-save-error", path, e))?;
    if metadata.permissions().readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            own_with("macros-read-only", &[("path", &path.display().to_string())]),
        ));
    }
    let check = || -> io::Result<()> {
        static NEXT_PROBE: AtomicU64 = AtomicU64::new(1);
        let parent = target.directory();
        loop {
            let id = NEXT_PROBE.fetch_add(1, Ordering::Relaxed);
            let probe = parent.join(format!(".macro-save-check-{}-{id}", std::process::id()));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&probe)
            {
                Ok(file) => {
                    drop(file);
                    return std::fs::remove_file(probe);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    };
    check().map_err(|e| path_error("macros-save-error", path, e))?;
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
    let Some(state) = window_state(ctx, app, open) else {
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
    let owners_id = egui::Id::new("macro-viewport-owners");
    ctx.data_mut(|d| {
        let mut owners = d
            .get_temp::<Vec<(egui::ViewportId, egui::Id)>>(owners_id)
            .unwrap_or_default();
        owners.retain(|(viewport, id)| {
            let keep = *id != crate::window_id(*viewport, ("shared-macros", app));
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
    fn state_keeps_one_runner_per_window_and_stop_ends_it() {
        let (_temp, roots, entry) = runner_fixture();
        let mut state = State::new("ced");
        state.engine.roots = Some(roots);
        let ctx = egui::Context::default();
        state.run(entry.clone(), &ctx);
        state.run(entry.clone(), &ctx);
        assert_eq!(state.engine.runner.status(), Status::Running);
        assert!(state.run_message.is_some());
        let other_window = State::new("ced");
        assert_eq!(other_window.engine.runner.status(), Status::Idle);
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
            matches!(&result, Ok((_, _, None))),
            "Ctrl+Enter only saves while already running"
        );
        state.saved(result, &ctx);
        assert!(!state.document.as_ref().unwrap().is_dirty());
        state.engine.runner.stop();
        wait_until(|| state.engine.runner.status() != Status::Running);
        assert!(matches!(state.engine.runner.status(), Status::Exited(_)));
    }

    #[cfg(unix)]
    #[test]
    fn late_output_does_not_hide_a_live_run() {
        let (_temp, roots, entry) = runner_fixture();
        let mut state = State::new("ced");
        state.engine.roots = Some(roots);
        state.run(entry, &egui::Context::default());
        state.retained = Some(Finished {
            title: "Earlier run".into(),
            output: "earlier output".into(),
            status: "Earlier run done".into(),
        });
        assert_eq!(state.engine.running.as_ref().unwrap().title, "Wait");
        assert_eq!(state.displayed_output(), state.engine.runner.output().text);
        assert_ne!(state.displayed_output(), "earlier output");
        state.engine.runner.stop();
        wait_until(|| state.engine.runner.status() != Status::Running);
        assert_eq!(state.displayed_output(), "earlier output");
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
        state.engine.roots = Some(roots);
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
            assert_eq!(
                payload["actions"][0],
                serde_json::json!({"id": "output", "label": "Show output"})
            );
        }
        assert_eq!(
            notification("ced", "Tidy", &Status::Exited(Some(0)), "")["body"],
            "done"
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
    fn menu_groups_app_before_global_limits_rows_and_offers_stop() {
        let catalogue = Catalogue {
            entries: (0..20)
                .map(|n| Entry {
                    id: format!("id-{n}"),
                    title: format!("Title {n}"),
                    path: format!("{n}.mix").into(),
                    tier: if n % 2 == 0 {
                        Tier::BuiltinGlobal
                    } else {
                        Tier::UserApp
                    },
                    shadows: vec![],
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
        state.engine.roots = Some(Roots {
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
        state.engine.roots = Some(roots);
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
        };
        let destination = folder.join("builtin.mix");
        let mut state = State::new("ced");
        state.engine.roots = Some(roots);
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
            state.engine.roots = Some(roots.clone());
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
    fn symlink_checks_and_saves_use_the_target_directory_in_both_directions() {
        use std::os::unix::fs::symlink;
        for target_readonly in [false, true] {
            let (_temp, roots, mut entry) = runner_fixture();
            let links = roots.folder("ced", Tier::UserApp).unwrap();
            let targets = roots.user.join("targets");
            std::fs::create_dir(&targets).unwrap();
            let target = targets.join("target.mix");
            std::fs::write(&target, "original").unwrap();
            entry.path = links.join("linked.mix");
            symlink(&target, &entry.path).unwrap();
            let resolved = editor::local::save_target(&entry.path).unwrap();
            assert_eq!(resolved.path(), target.canonicalize().unwrap().as_path());
            assert_eq!(
                resolved.directory(),
                targets.canonicalize().unwrap().as_path()
            );
            let Some(_readonly) =
                ReadOnlyParent::new(if target_readonly { &targets } else { &links })
            else {
                return;
            };
            if target_readonly {
                let mut state = State::new("ced");
                state.engine.roots = Some(roots);
                state.loaded = true;
                state.edit(entry.clone());
                let mut h = permission_harness(state);
                wait_until(|| {
                    h.run_steps(1);
                    h.state().pending.is_none()
                });
                let error = h.state().error.as_ref().unwrap();
                assert!(error.contains(&entry.path.display().to_string()));
                assert!(error.to_lowercase().contains("permission denied"));
                let _ = h.get_by_label(error);
                assert!(!h.state().editor_open);
                assert!(h.state().document.is_none());
                // Both save APIs must fail in exactly the directory the probe
                // refused, even though the link's own directory is writable.
                let mut doc = editor::local::Document::open(&entry.path).unwrap();
                assert!(doc.save().is_err());
                assert!(doc.save_snapshot().unwrap().write().is_err());
                assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
            } else {
                let mut doc = open_macro(&entry.path).unwrap();
                doc.command(
                    editor::EditCommand::Insert("edit ".into()),
                    &editor::View::default(),
                )
                .unwrap();
                doc.save().unwrap();
                let saved = doc.save_snapshot().unwrap().write().unwrap();
                assert!(doc.complete_save(&saved));
                assert_eq!(std::fs::read_to_string(&target).unwrap(), "edit original");
                assert!(
                    entry
                        .path
                        .symlink_metadata()
                        .unwrap()
                        .file_type()
                        .is_symlink()
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn read_only_macro_file_is_refused_inline_in_a_writable_directory() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, roots, entry) = runner_fixture();
        std::fs::set_permissions(&entry.path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let before = std::fs::read(&entry.path).unwrap();
        let mut state = State::new("ced");
        state.engine.roots = Some(roots);
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
        let mut document = open_macro(&entry.path).unwrap();
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
        state.engine.roots = Some(roots);
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
        state.engine.roots = Some(roots);
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
                    state.engine.roots = Some(Roots {
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
        let roots = runtime(&h).lock().unwrap().engine.roots.clone().unwrap();
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
        };
        runtime(&h).lock().unwrap().engine.catalogue.entries = vec![entry("alpha")];
        h.get_by_label("Macros").click();
        h.run_steps(5);
        h.event(egui::Event::Text("a".into()));
        h.run_steps(1);
        let (tx, rx) = mpsc::channel();
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
        assert_eq!(state.engine.runner.status(), Status::Idle);
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
        let weak = Arc::downgrade(&states[1]);
        states.pop();
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {
            let _ = window_state(&ctx, "ced", false);
        });
        out.textures_delta.clear();
        assert!(weak.upgrade().is_none());
    }
}
