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
use std::sync::{Arc, Mutex, mpsc};
use std::sync::atomic::{AtomicU64, Ordering};
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
    pub catalogue: Catalogue,
    pub runner: Runner,
    app: String,
    roots: Option<Roots>,
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
    loaded: bool,
    error: Option<String>,
    document: Option<editor::local::Document>,
    editor_entry: Option<Entry>,
    editor_open: bool,
    editor_focus: Option<egui::Id>,
    running: Option<Entry>,
    retained: Option<Finished>,
    show_retained: bool,
}

impl State {
    pub fn new(app: &str) -> Self {
        let (notify_tx, notifications) = mpsc::channel();
        Self {
            open: false,
            catalogue: Catalogue::default(),
            runner: Runner::default(),
            app: app.into(),
            roots: Roots::resolve(),
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
            loaded: false,
            error: None,
            document: None,
            editor_entry: None,
            editor_open: false,
            editor_focus: None,
            running: None,
            retained: None,
            show_retained: false,
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
        let Some(roots) = self.roots.clone() else {
            self.error = Some(own("macros-no-home"));
            self.editor_open = true;
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
        if self.runner.status() == Status::Running {
            self.run_message = Some(own("macros-already-running"));
            self.editor_open = true;
            return;
        }
        // Completion can race a menu click between frames. Retain and report
        // the previous terminal run before Runner resets its output buffer.
        self.finish_run(ctx);
        self.run_message = None;
        if let Some(roots) = &self.roots {
            // Thread-spawn failure sets a terminal status too; polling it
            // follows the same notification path as a child failing to start.
            let _ = self.runner.start(roots, &self.app, &entry);
            self.running = Some(entry);
            self.retained = None;
            self.show_retained = false;
        } else {
            self.error = Some(own("macros-no-home"));
            self.editor_open = true;
        }
    }

    fn finish_run(&mut self, ctx: &egui::Context) {
        let status = self.runner.status();
        if status == Status::Running {
            return;
        }
        if let Some(entry) = self.running.take() {
            self.run_message = None;
            let output = self.runner.output().text;
            let payload = notification(&self.app, &entry.title, &status, &output);
            let finished = Finished {
                title: entry.title,
                output,
                status: payload["summary"].as_str().unwrap_or_default().into(),
            };
            self.retained = Some(finished.clone());
            notifications::post(&self.app, self.window, payload, finished, self.notify_tx.clone(), &self.lifetime, ctx);
        }
    }

    fn create_title(&mut self) {
        if self.creation.is_some() { return; }
        let Some(title) = self.title.clone() else { return; };
        let Some(roots) = self.roots.clone() else {
            self.creation_error = Some(own("macros-no-home"));
            return;
        };
        self.creation_error = None;
        let app = self.app.clone();
        let (tx, rx) = mpsc::channel();
        self.creation = Some(rx);
        std::thread::spawn(move || {
            let result = (|| {
                let path = roots.create_from_title(&app, &title)?;
                let doc = editor::local::Document::open(path).map_err(io::Error::other)?;
                Ok((Catalogue::scan(&roots, &app)?, doc))
            })().map_err(|e: io::Error| e.to_string());
            let _ = tx.send(result);
        });
    }

    fn created(&mut self, result: Creation, ctx: &egui::Context) {
        let viewport = ctx.viewport_id();
        self.creation = None;
        match result {
            Ok((catalogue, doc)) => {
                self.catalogue = catalogue;
                self.editor_entry = self.catalogue.entries.iter().find(|e| Some(e.path.as_path()) == doc.path()).cloned();
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
                ctx.data_mut(|d| d.insert_temp(crate::window_id(viewport, "macro-title-focused"), false));
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
                roots.copy_to_user(app, &entry)?
            };
            Ok((
                Some(editor::local::Document::open(path).map_err(io::Error::other)?),
                None,
            ))
        });
    }

    fn can_replace_document(&mut self) -> bool {
        if self.document.as_ref().is_some_and(|doc| doc.is_dirty()) {
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
        if let Some(rx) = &self.creation {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(own("macros-worker-failed"))),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result { self.created(result, ctx); }
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
                        self.catalogue = catalogue;
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
                        self.editor_open = true;
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
            self.show_retained = true;
            self.editor_open = true;
        }
        ctx.data_mut(|d| {
            d.insert_temp(
                crate::window_id(viewport, "macro-busy"),
                self.runner.status() == Status::Running,
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
                            let enter = ui.input_mut(|i| {
                                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                            });
                            let escape = ui.input_mut(|i| {
                                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                            });
                            let field = ui.add_enabled(self.creation.is_none(),
                                egui::TextEdit::singleline(title)
                                    .id(crate::window_id(viewport, "macro-title"))
                                    .hint_text(own("macros-title")),
                            );
                            if let Some(error) = &self.creation_error { ui.label(error); }
                            if !ctx
                                .data(|d| d.get_temp::<bool>(crate::window_id(viewport, "macro-title-focused")))
                                .unwrap_or(false)
                            {
                                field.request_focus();
                                ctx.data_mut(|d| {
                                    d.insert_temp(crate::window_id(viewport, "macro-title-focused"), true)
                                });
                            }
                            (field, enter, escape)
                        });
                        let (field, enter, escape) = shown.inner;
                        let outside = ctx.input(|i| {
                            i.pointer.any_pressed()
                                && i.pointer
                                    .interact_pos()
                                    .is_some_and(|p| !shown.response.rect.contains(p))
                        });
                        if enter && !title.trim().is_empty() {
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
                    let (model, indices) = macro_menu(
                        &self.catalogue,
                        self.running.as_ref().map(|e| e.title.as_str()),
                    );
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
                "macro.stop" => self.runner.stop(),
                _ => {}
            }
        }
        if !self.open {
            ctx.memory_mut(|m| m.surrender_focus(crate::window_id(viewport, "macro-title")));
            self.title = None;
        }
        menu::anchored_open(ctx, self.open);
        self.show_editor(ctx);
        // Poll even with the menu closed: a fast failed start and later
        // notification actions must still reach this window.
        if self.runner.status() == Status::Running || self.pending.is_some() || self.creation.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
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
                    .data_mut(|d| d.remove_temp::<bool>(crate::window_id(viewport, "macro-save-run")))
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
                if let Some(message) = &self.run_message { ui.label(message); }
                let output = if self.runner.status() == Status::Running && !self.show_retained {
                    self.runner.output().text
                } else {
                    self.retained
                        .as_ref()
                        .map(|r| r.output.clone())
                        .unwrap_or_default()
                };
                if let Some(last) = &self.retained {
                    ui.label(&last.status).on_hover_text(&last.title);
                }
                if !self.show_retained
                    && let Some(running) = &self.running
                {
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
        self.editor_open = open;
        if !open {
            if let Some(id) = self.editor_focus {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
            ctx.data_mut(|d| d.remove::<Option<egui::Id>>(crate::window_id(viewport, "macro-editor-focus")));
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
        if run && self.runner.status() == Status::Running {
            self.run_message = Some(own("macros-already-running"));
            run = false;
        }
        if save || requested {
            self.save_request = Some(self.save_request.unwrap_or(false) || run);
        }
        if self.pending.is_none() && self.document.is_some()
            && let Some(run) = self.save_request.take()
            && let Some(mut doc) = self.document.take()
        {
            let entry = if run { self.editor_entry.clone() } else { None };
            let Some(roots) = self.roots.clone() else {
                self.document = Some(doc);
                return;
            };
            let app = self.app.clone();
            let (tx, rx) = mpsc::channel();
            self.pending = Some(rx);
            std::thread::spawn(move || {
                let result = doc
                    .save()
                    .map_err(|e| e.to_string())
                    .and_then(|()| Catalogue::scan(&roots, &app).map_err(|e| e.to_string()));
                let completion = match result {
                    Ok(catalogue) => Ok((catalogue, Some(doc), entry)),
                    Err(error) => Err((error, Some(doc))),
                };
                let _ = tx.send(completion);
            });
        }
    }
}

impl Drop for State {
    fn drop(&mut self) { notifications::forget(self.window); }
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
            let mut item = Row::command(SLOTS[indices.len()], &entry.title, None, running.is_none());
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
    let Some(state) = window_state(ctx, app, open) else { return false; };
    let mut state = state.lock().unwrap();
    state.open = open;
    state.show(ctx);
    state.open != open
}

type WindowState = Arc<Mutex<State>>;

fn window_state(ctx: &egui::Context, app: &str, open: bool) -> Option<WindowState> {
    let viewport = ctx.viewport_id();
    let owners_id = egui::Id::new("macro-viewport-owners");
    let mut owners = ctx.data(|d| d.get_temp::<Vec<(egui::ViewportId, egui::Id)>>(owners_id)).unwrap_or_default();
    owners.retain(|(viewport, id)| {
        let present = ctx.input(|i| i.raw.viewports.contains_key(viewport));
        if !present { ctx.data_mut(|d| d.remove::<WindowState>(*id)); }
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
        assert!(menu.entries.iter().filter_map(MenuEntry::row)
            .filter(|r| r.id.is_some_and(|id| SLOTS.contains(&id))).all(|r| !r.enabled));
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
    fn state_keeps_one_runner_per_window_and_stop_ends_it() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("wait.mix");
        std::fs::write(&path, "while true\nsleep(1)\nend\n").unwrap();
        let mut state = State::new("ced");
        let Some(roots) = &mut state.roots else {
            return;
        };
        if !roots.mix.is_file() {
            return;
        }
        roots.home = temp.path().into();
        let entry = Entry {
            id: "wait".into(),
            title: "First".into(),
            tier: Tier::UserApp,
            path,
            shadows: vec![],
        };
        let ctx = egui::Context::default();
        state.run(entry.clone(), &ctx);
        let mut other = entry;
        other.title = "Second".into();
        state.run(other, &ctx);
        assert_eq!(state.running.as_ref().unwrap().title, "First");
        assert_eq!(state.runner.status(), Status::Running);
        let path = state.running.as_ref().unwrap().path.clone();
        let text = "-- saved while running\nwhile true\nsleep(1)\nend\n";
        let mut doc = editor::local::Document::open(&path).unwrap();
        doc.set_contents(text).unwrap();
        state.document = Some(doc);
        state.save_editor(false, true);
        assert_eq!(state.run_message.as_deref(), Some(own("macros-already-running").as_str()));
        let result = state.pending.take().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
        let Ok((_, doc, run)) = result else { panic!("save failed") };
        assert!(run.is_none(), "Ctrl+Enter saves without queueing a second run");
        assert!(!doc.unwrap().is_dirty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let another_window = State::new("ced");
        assert_eq!(another_window.runner.status(), Status::Idle);
        state.runner.stop();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while state.runner.status() == Status::Running {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(state.runner.status(), Status::Exited(_)));
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
        state.roots = Some(Roots { user, builtin: temp.path().join("builtin"), home: temp.path().into(), mix: "/opt/mixos/bin/mix".into() });
        state.create_title();
        let result = state.creation.take().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(result.is_err());
        state.created(result, &egui::Context::default());
        assert_eq!(state.title.as_deref(), Some("Tidy imports"));
        assert!(state.open);
        assert!(state.creation_error.is_some());
        assert!(!state.editor_open);
    }

    #[test]
    fn viewports_have_separate_state_and_closed_viewports_are_released() {
        let ctx = egui::Context::default();
        let second = egui::ViewportId::from_hash_of("second");
        let mut states = Vec::new();
        let mut ids = Vec::new();
        for viewport in [egui::ViewportId::ROOT, second] {
            let mut input = egui::RawInput { viewport_id: viewport, ..Default::default() };
            input.viewports.insert(second, Default::default());
            let mut out = ctx.run_ui(input, |_| {
                states.push(window_state(&ctx, "ced", true).unwrap());
                ids.push(crate::window_id(viewport, "macro-editor"));
            });
            out.textures_delta.clear();
        }
        assert!(!Arc::ptr_eq(&states[0], &states[1]));
        assert_ne!(ids[0], ids[1]);
        assert_ne!(states[0].lock().unwrap().window, states[1].lock().unwrap().window);
        let weak = Arc::downgrade(&states[1]);
        states.pop();
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| { let _ = window_state(&ctx, "ced", false); });
        out.textures_delta.clear();
        assert!(weak.upgrade().is_none());
    }
}
