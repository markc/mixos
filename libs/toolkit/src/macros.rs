// SPDX-License-Identifier: MIT OR Apache-2.0
//! Per-window macro menu, editor and background runtime.
use crate::menu::{self, Entry as MenuEntry, Menu, Row};
use crate::strings::{own, own_with};
use ::macros::{Catalogue, Entry, Roots, Runner, Status, Tier};
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, Weak, mpsc};
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
    title: Option<String>,
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
            title: None,
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

    fn run(&mut self, entry: Entry) {
        if self.runner.status() == Status::Running {
            return;
        }
        // Completion can race a menu click between frames. Retain and report
        // the previous terminal run before Runner resets its output buffer.
        self.finish_run();
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

    fn finish_run(&mut self) {
        let status = self.runner.status();
        if status == Status::Running {
            return;
        }
        if let Some(entry) = self.running.take() {
            let output = self.runner.output().text;
            let payload = notification(&self.app, &entry.title, &status, &output);
            let finished = Finished {
                title: entry.title,
                output,
                status: payload["summary"].as_str().unwrap_or_default().into(),
            };
            self.retained = Some(finished.clone());
            let tx = self.notify_tx.clone();
            let lifetime = Arc::downgrade(&self.lifetime);
            std::thread::spawn(move || notify_output(payload, finished, tx, lifetime));
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
                            self.run(entry);
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
        self.finish_run();
        while let Ok(finished) = self.notifications.try_recv() {
            self.retained = Some(finished);
            self.show_retained = true;
            self.editor_open = true;
        }
        ctx.data_mut(|d| {
            d.insert_temp(
                egui::Id::new("macro-busy"),
                self.runner.status() == Status::Running,
            )
        });
        if self.open && !self.loaded && self.pending.is_none() {
            self.reload();
        }
        let anchor = ctx
            .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("macro-anchor")))
            .unwrap_or_else(|| {
                egui::Rect::from_min_size(ctx.content_rect().left_top(), egui::Vec2::ZERO)
            });
        let mut selected = None;
        egui::Area::new(egui::Id::new("macro-menu-owner"))
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
                            let field = ui.add(
                                egui::TextEdit::singleline(title)
                                    .id(egui::Id::new("macro-title"))
                                    .hint_text(own("macros-title")),
                            );
                            if !ctx
                                .data(|d| d.get_temp::<bool>(egui::Id::new("macro-title-focused")))
                                .unwrap_or(false)
                            {
                                field.request_focus();
                                ctx.data_mut(|d| {
                                    d.insert_temp(egui::Id::new("macro-title-focused"), true)
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
                        if escape || outside || selected.is_some() {
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
                        egui::Id::new("macro-nav"),
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
                                self.run(entry);
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
                        self.open = true;
                        ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("macro-title-focused"), false)
                        });
                    }
                }
                "macro.create" => {
                    let title = self.title.take().unwrap();
                    self.work(move |roots, app| {
                        let path = roots.create_from_title(app, &title)?;
                        Ok((
                            Some(editor::local::Document::open(path).map_err(io::Error::other)?),
                            None,
                        ))
                    });
                }
                "macro.workshop" => self.folder(),
                "macro.stop" => self.runner.stop(),
                _ => {}
            }
        }
        if !self.open {
            ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("macro-title")));
            self.title = None;
        }
        menu::anchored_open(ctx, self.open);
        self.show_editor(ctx);
        // Poll even with the menu closed: a fast failed start and later
        // notification actions must still reach this window.
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn show_editor(&mut self, ctx: &egui::Context) {
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
            .id(egui::Id::new("macro-editor"))
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
                    .data_mut(|d| d.remove_temp::<bool>(egui::Id::new("macro-save")))
                    .unwrap_or(false);
                run |= ctx
                    .data_mut(|d| d.remove_temp::<bool>(egui::Id::new("macro-save-run")))
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
                                    egui::Id::new("macro-editor-focus"),
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
            ctx.data_mut(|d| d.remove::<Option<egui::Id>>(egui::Id::new("macro-editor-focus")));
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
        if (save || run)
            && self.pending.is_none()
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

pub(crate) fn keyboard(ctx: &egui::Context) {
    let focus = ctx
        .data(|d| d.get_temp::<Option<egui::Id>>(egui::Id::new("macro-editor-focus")))
        .flatten();
    if focus.is_some_and(|id| ctx.memory(|m| m.has_focus(id))) && !menu::is_open(ctx) {
        for (key, flag) in [
            (egui::Key::S, "macro-save"),
            (egui::Key::Enter, "macro-save-run"),
        ] {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, key)) {
                ctx.data_mut(|d| d.insert_temp(egui::Id::new(flag), true));
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
            let mut item = Row::command(SLOTS[indices.len()], &entry.title, None, true);
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
    })
}

// Mix is the native Bus client. Raw literals keep titles/output as data.
fn bus(verb: &str, body: &serde_json::Value) -> io::Result<serde_json::Value> {
    let raw = body.to_string().replace('\\', "\\\\").replace('\'', "\\'");
    let source = format!(
        "send notify {verb} body='{raw}' timeout=2; if $rc != 0 then raise(\"NOTIFY_FAILED\", to_string($result)) end; if type($result) == \"string\" then print($result) else print(json_encode($result)) end"
    );
    let output = Command::new("/opt/mixos/bin/mix")
        .args(["-c", &source])
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(io::Error::other)
}

fn notify_output(
    payload: serde_json::Value,
    finished: Finished,
    tx: mpsc::Sender<Finished>,
    lifetime: Weak<()>,
) {
    notify_output_with(payload, finished, tx, lifetime, bus);
}

fn notify_output_with(
    payload: serde_json::Value,
    finished: Finished,
    tx: mpsc::Sender<Finished>,
    lifetime: Weak<()>,
    mut call: impl FnMut(&str, &serde_json::Value) -> io::Result<serde_json::Value>,
) {
    let sent = call("notify.send", &payload).and_then(|reply| {
        reply["id"]
            .as_u64()
            .ok_or_else(|| io::Error::other("missing notification id"))
    });
    let Ok(id) = sent else {
        let _ = tx.send(finished);
        return;
    };
    // Recorded outcomes make polling race-free even for immediate answers.
    while lifetime.upgrade().is_some() {
        std::thread::sleep(Duration::from_millis(500));
        match call("notify.get", &serde_json::json!({"id": id})) {
            Ok(reply) if reply["outcome"]["action"] == "output" => {
                let _ = tx.send(finished);
                return;
            }
            Ok(reply) if !reply["outcome"].is_null() => return,
            Ok(_) => {}
            Err(_) => {
                let _ = tx.send(finished);
                return;
            }
        }
    }
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
    let id = egui::Id::new(("shared-macros", app));
    let state: Option<Arc<Mutex<State>>> = ctx.data(|d| d.get_temp(id));
    if !open && state.is_none() {
        return false;
    }
    let state = state.unwrap_or_else(|| {
        let state = Arc::new(Mutex::new(State::new(app)));
        ctx.data_mut(|d| d.insert_temp(id, state.clone()));
        state
    });
    let mut state = state.lock().unwrap();
    state.open = open;
    state.show(ctx);
    state.open != open
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
    fn absent_notifyd_opens_the_output_without_opening_the_menu() {
        let mut state = State::new("ced");
        let finished = Finished {
            title: "Tidy".into(),
            output: "retained output".into(),
            status: "Tidy done".into(),
        };
        notify_output_with(
            serde_json::json!({}),
            finished,
            state.notify_tx.clone(),
            Arc::downgrade(&state.lifetime),
            |_, _| Err(io::Error::other("absent")),
        );
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| state.show(&ctx));
        out.textures_delta.clear();
        assert!(state.editor_open);
        assert!(!state.open);
        assert_eq!(state.retained.as_ref().unwrap().output, "retained output");
    }

    #[test]
    fn show_output_matches_the_notification_id_and_retains_that_runs_output() {
        let state = State::new("ced");
        let finished = Finished {
            title: "Earlier".into(),
            output: "earlier output".into(),
            status: "Earlier done".into(),
        };
        notify_output_with(
            serde_json::json!({}),
            finished,
            state.notify_tx.clone(),
            Arc::downgrade(&state.lifetime),
            |verb, body| {
                if verb == "notify.send" {
                    Ok(serde_json::json!({"id": 42}))
                } else {
                    assert_eq!(body["id"], 42);
                    Ok(serde_json::json!({"outcome": {"action": "output"}}))
                }
            },
        );
        assert_eq!(
            state.notifications.try_recv().unwrap().output,
            "earlier output"
        );
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
        state.run(entry.clone());
        let mut other = entry;
        other.title = "Second".into();
        state.run(other);
        assert_eq!(state.running.as_ref().unwrap().title, "First");
        assert_eq!(state.runner.status(), Status::Running);
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
}
