// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A path-only, multi-file chooser. The caller owns it until it answers.
//! All filesystem work runs on detached workers; abandoning a slow directory
//! never waits for its worker or lets its eventual answer replace a newer one.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{Context, Id, Key, Modifiers};

use crate::dialog::{Choice, Dialog, Role};
use crate::strings::{own, own_with};
use crate::{Icon, button, field, icons, toggle};

const ENTRY_LIMIT: usize = 10_000;
const WORKER_LIMIT: usize = 8;
static WORKERS: AtomicUsize = AtomicUsize::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Picked(Vec<PathBuf>),
    Save { path: PathBuf, replace: bool },
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Multiple,
    Single,
    Save,
}

#[derive(Clone, Debug)]
struct Entry {
    path: PathBuf,
    name: String,
    directory: bool,
    size: u64,
    modified: Option<SystemTime>,
}

struct Listing {
    directory: PathBuf,
    entries: Vec<Entry>,
    capped: bool,
}

enum Request {
    Open(PathBuf),
    Pick(Vec<PathBuf>),
    Save(PathBuf),
}

enum Answer {
    Listed(Listing),
    Picked(Vec<PathBuf>),
    Save(PathBuf, bool),
}

#[derive(Default)]
struct Selection {
    files: BTreeSet<usize>,
    cursor: Option<usize>,
    anchor: Option<usize>,
}

impl Selection {
    fn click(&mut self, at: usize, ctrl: bool, shift: bool, entries: &[Entry]) {
        if at >= entries.len() {
            return;
        }
        self.cursor = Some(at);
        if shift {
            let anchor = self.anchor.unwrap_or(at);
            if !ctrl {
                self.files.clear();
            }
            self.files
                .extend((anchor.min(at)..=anchor.max(at)).filter(|&i| !entries[i].directory));
            self.anchor = Some(anchor);
        } else {
            self.anchor = Some(at);
            if !ctrl {
                self.files.clear();
            }
            if !entries[at].directory && (!ctrl || !self.files.remove(&at)) {
                self.files.insert(at);
            }
        }
    }

    fn all(&mut self, entries: &[Entry]) {
        self.files = entries
            .iter()
            .enumerate()
            .filter_map(|(i, entry)| (!entry.directory).then_some(i))
            .collect();
    }

    fn paths(&self, entries: &[Entry]) -> Vec<PathBuf> {
        self.files
            .iter()
            .map(|&i| entries[i].path.clone())
            .collect()
    }
}

/// A reusable chooser of regular files (following symlinks), with its own
/// stable widget ids and modal layer. No file bytes are read or retained.
pub struct FileChooser {
    id: Id,
    title: String,
    confirm: String,
    mode: Mode,
    name: String,
    focus_name: bool,
    replace: Option<PathBuf>,
    directory: PathBuf,
    path: String,
    browsing: Option<PathBuf>,
    hidden: bool,
    entries: Vec<Entry>,
    row_ids: std::collections::HashSet<Id>,
    selection: Selection,
    capped: bool,
    error: Option<String>,
    pending: Option<Receiver<Result<Answer, String>>>,
    started: bool,
    scroll_to_cursor: bool,
}

impl FileChooser {
    pub fn new(
        start: impl Into<PathBuf>,
        title: impl Into<String>,
        confirm: impl Into<String>,
    ) -> Self {
        let directory = start.into();
        Self {
            id: Id::unique((
                "toolkit.file-chooser",
                NEXT_ID.fetch_add(1, Ordering::Relaxed),
            )),
            title: title.into(),
            confirm: confirm.into(),
            mode: Mode::Multiple,
            name: String::new(),
            focus_name: false,
            replace: None,
            path: directory.display().to_string(),
            browsing: None,
            directory,
            hidden: false,
            entries: Vec::new(),
            row_ids: std::collections::HashSet::new(),
            selection: Selection::default(),
            capped: false,
            error: None,
            pending: None,
            started: false,
            scroll_to_cursor: false,
        }
    }

    /// Choose exactly one existing file.
    pub fn single(
        start: impl Into<PathBuf>,
        title: impl Into<String>,
        confirm: impl Into<String>,
    ) -> Self {
        Self {
            mode: Mode::Single,
            ..Self::new(start, title, confirm)
        }
    }

    /// Choose a destination; no file is written by the chooser. A replacement
    /// is returned only after the person explicitly accepts its question.
    pub fn save(
        start: impl Into<PathBuf>,
        title: impl Into<String>,
        confirm: impl Into<String>,
    ) -> Self {
        Self {
            mode: Mode::Save,
            focus_name: true,
            ..Self::new(start, title, confirm)
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn can_confirm(&self) -> bool {
        self.pending.is_none()
            && if self.mode == Mode::Save {
                save_path(&self.directory, &self.name).is_ok()
            } else {
                !self.selection.files.is_empty()
            }
    }

    /// Show above the current modal; return once the person picks or cancels.
    pub fn show(&mut self, ctx: &Context) -> Option<Outcome> {
        if !self.started {
            self.started = true;
            crate::menu::close(ctx);
            self.open(ctx, self.directory.clone());
        }
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(answer) => {
                    self.pending = None;
                    match answer {
                        Ok(Answer::Picked(paths)) => {
                            if self.mode == Mode::Save {
                                if let Some(path) = paths.first() {
                                    self.name = path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned();
                                    self.open(
                                        ctx,
                                        path.parent().unwrap_or(&self.directory).to_owned(),
                                    );
                                }
                                return None;
                            }
                            Dialog::reset(ctx, self.id);
                            return Some(Outcome::Picked(paths));
                        }
                        Ok(Answer::Save(path, true)) => {
                            self.replace = Some(path);
                            // The filename field belongs to the covered chooser.
                            // Let the replacement question own Enter and Escape.
                            ctx.memory_mut(|memory| {
                                if let Some(id) = memory.focused() {
                                    memory.surrender_focus(id);
                                }
                            });
                        }
                        Ok(Answer::Save(path, false)) => {
                            Dialog::reset(ctx, self.id);
                            return Some(Outcome::Save {
                                path,
                                replace: false,
                            });
                        }
                        Ok(Answer::Listed(listing)) => {
                            self.browsing = None;
                            self.directory = listing.directory;
                            if !ctx.memory(|m| m.has_focus(self.id.with("path"))) {
                                self.path = self.directory.display().to_string();
                            }
                            self.entries = listing.entries;
                            self.row_ids.clear();
                            self.capped = listing.capped;
                            self.selection = Selection::default();
                        }
                        Err(error) => {
                            self.browsing = None;
                            self.error = Some(error);
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.browsing = None;
                    self.pending = None;
                    self.error = Some(own("file-chooser-worker-stopped"));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let layer = egui::LayerId::new(egui::Order::Foreground, self.id);
        // Raising both areas gives them equal priority and can leave a
        // reopened replacement question behind the chooser.
        if self.replace.is_none() {
            ctx.move_to_top(layer);
        }
        let choices = vec![
            Choice::new(&self.confirm, Role::Default).enabled(self.can_confirm()),
            Choice::new(own("file-chooser-cancel"), Role::Cancel),
        ];
        let shown = Dialog::new(self.id, &self.title)
            .enabled(self.replace.is_none())
            .resizable(
                ctx.content_rect().size() * 0.7,
                ctx.content_rect().size() * 0.4,
            )
            .buttons(choices)
            .show(ctx, |ui| self.body(ui));
        if shown.dismissed || shown.chosen == Some(1) {
            Dialog::reset(ctx, self.id);
            return Some(Outcome::Cancelled);
        }
        if shown.chosen == Some(0) {
            self.pick(ctx);
        }
        if let Some(path) = self.replace.clone() {
            let id = self.id.with("replace");
            ctx.move_to_top(egui::LayerId::new(egui::Order::Foreground, id));
            let shown = Dialog::new(id, own("file-chooser-replace-title"))
                .buttons(vec![
                    Choice::new(own("file-chooser-replace"), Role::Default),
                    Choice::new(own("file-chooser-cancel"), Role::Cancel),
                ])
                .show(ctx, |ui| {
                    ui.label(own_with(
                        "file-chooser-replace-body",
                        &[("path", &path.display().to_string())],
                    ));
                });
            if shown.chosen.is_some() || shown.dismissed {
                Dialog::reset(ctx, id);
                self.replace = None;
                if shown.chosen == Some(0) {
                    Dialog::reset(ctx, self.id);
                    return Some(Outcome::Save {
                        path,
                        replace: true,
                    });
                }
            }
        }
        None
    }

    fn body(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let path_id = self.id.with("path");
        let path_focused = ctx.memory(|m| m.has_focus(path_id));
        // Buttons and fields own their keys. Only an unfocused chooser or
        // one of the list rows delegates navigation and activation to us.
        let list_focused = ctx
            .memory(|m| m.focused())
            .is_none_or(|id| self.row_ids.contains(&id));
        let list_keys = ui.is_enabled() && list_focused && !egui::Popup::is_any_open(&ctx);
        // Consume before TextEdit: Enter otherwise gives up focus and can
        // also activate the dialog's default button.
        let submit = ui.is_enabled()
            && path_focused
            && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        ui.add(
            field::text_edit(
                &mut self.path,
                &own("file-chooser-path"),
                ui.available_width(),
            )
            .id(path_id),
        );
        if submit {
            self.open(&ctx, resolve_path(&self.directory, &self.path, &home()));
        }
        if self.mode == Mode::Save {
            let id = self.id.with("name");
            let submit = ui.is_enabled()
                && ctx.memory(|m| m.has_focus(id))
                && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
            let response = ui.add(
                field::text_edit(
                    &mut self.name,
                    &own("file-chooser-name"),
                    ui.available_width(),
                )
                .id(id),
            );
            if std::mem::take(&mut self.focus_name) {
                response.request_focus();
            }
            if submit && self.pending.is_none() {
                self.pick(&ctx);
            }
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.browsing
                        .as_ref()
                        .unwrap_or(&self.directory)
                        .parent()
                        .is_some(),
                    button::PushButton::secondary(own("file-chooser-up")),
                )
                .clicked()
            {
                self.up(&ctx);
            }
            if ui
                .add(button::PushButton::secondary(own("file-chooser-home")))
                .clicked()
            {
                self.open(&ctx, home());
            }
            if ui
                .add(toggle::Checkbox::new(
                    &mut self.hidden,
                    own("file-chooser-hidden"),
                ))
                .changed()
            {
                self.open(&ctx, self.directory.clone());
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if self.pending.is_some() {
            if list_keys && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Backspace)) {
                self.up(&ctx);
            }
            ui.weak(own("file-chooser-loading"));
            return;
        }
        let mut activate = false;
        let mut go_up = false;
        if list_keys {
            ui.input_mut(|i| {
                if i.consume_key(Modifiers::COMMAND, Key::A) && self.mode == Mode::Multiple {
                    self.selection.all(&self.entries);
                }
                let shift = i.modifiers.shift;
                let modifiers = if shift {
                    Modifiers::SHIFT
                } else {
                    Modifiers::NONE
                };
                let down = i.consume_key(modifiers, Key::ArrowDown);
                let up = i.consume_key(modifiers, Key::ArrowUp);
                if !self.entries.is_empty() && (down || up) {
                    let at = match self.selection.cursor {
                        Some(at) if down => (at + 1).min(self.entries.len() - 1),
                        Some(at) => at.saturating_sub(1),
                        None if down => 0,
                        None => self.entries.len() - 1,
                    };
                    self.selection.click(
                        at,
                        false,
                        shift && self.mode == Mode::Multiple,
                        &self.entries,
                    );
                    self.scroll_to_cursor = true;
                }
                activate = i.consume_key(Modifiers::NONE, Key::Enter);
                go_up = i.consume_key(Modifiers::NONE, Key::Backspace);
            });
        }
        let mut clicked = None;
        let mut double = None;
        self.row_ids.clear();
        egui::ScrollArea::vertical()
            .id_salt(self.id.with("list"))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (at, entry) in self.entries.iter().enumerate() {
                    ui.push_id(&entry.path, |ui| {
                        ui.horizontal(|ui| {
                            let chrome = crate::chrome::Chrome::of(ui.ctx());
                            ui.add(icons::image_of(
                                ui.ctx(),
                                if entry.directory {
                                    Icon::Folder
                                } else {
                                    Icon::File
                                },
                                field::height_of(ui.ctx()) * chrome.style.button_icon_share as f32,
                                chrome.palette.icon,
                            ));
                            let selected = self.selection.files.contains(&at)
                                || self.selection.cursor == Some(at);
                            let size =
                                (!entry.directory).then(|| crate::text::human_size(entry.size));
                            let date = entry.modified.and_then(modified_date);
                            // Size and date sit in fixed columns, sized for
                            // their widest forms, so every row lines up.
                            let font = egui::TextStyle::Body.resolve(ui.style());
                            let measure = |text: &str| {
                                ui.painter()
                                    .layout_no_wrap(
                                        text.to_owned(),
                                        font.clone(),
                                        chrome.palette.text,
                                    )
                                    .size()
                                    .x
                            };
                            let size_width = measure("1023 MB");
                            let date_width = measure("0000-00-00");
                            let height = field::height_of(ui.ctx());
                            let tail = size_width + date_width + 2.0 * ui.spacing().item_spacing.x;
                            let width = (ui.available_width() - tail).max(height);
                            let response = ui
                                .allocate_ui(egui::vec2(width, field::height_of(ui.ctx())), |ui| {
                                    // The name fills its cell, so size and date
                                    // start at the same x on every row.
                                    ui.set_min_width(width);
                                    ui.set_max_width(width);
                                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                                    button::selectable(ui, selected, &entry.name)
                                        .accessible_name(&entry.name)
                                        .on_hover_text(entry.path.display().to_string())
                                })
                                .inner;
                            self.row_ids.insert(response.id);
                            if self.scroll_to_cursor && self.selection.cursor == Some(at) {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                            if response.clicked() {
                                clicked = Some((at, ui.input(|i| i.modifiers)));
                            }
                            if response.double_clicked() {
                                double = Some(at);
                            }
                            ui.allocate_ui_with_layout(
                                egui::vec2(size_width, height),
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(size_width);
                                    if let Some(size) = size {
                                        ui.weak(size);
                                    }
                                },
                            );
                            ui.allocate_ui_with_layout(
                                egui::vec2(date_width, height),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.set_min_width(date_width);
                                    if let Some(date) = date {
                                        ui.weak(date).on_hover_text(own("file-chooser-modified"));
                                    }
                                },
                            );
                        });
                    });
                }
                if self.entries.is_empty() && self.error.is_none() {
                    ui.weak(own("file-chooser-empty"));
                }
                if self.capped {
                    ui.weak(own("file-chooser-capped"));
                }
            });
        if let Some((at, modifiers)) = clicked {
            self.selection.click(
                at,
                self.mode == Mode::Multiple && (modifiers.command || modifiers.ctrl),
                self.mode == Mode::Multiple && modifiers.shift,
                &self.entries,
            );
        }
        if self.mode == Mode::Save
            && let Some(at) = self.selection.cursor
            && !self.entries[at].directory
            && (clicked.is_some() || self.scroll_to_cursor)
        {
            self.name = self.entries[at].name.clone();
        }
        self.scroll_to_cursor = false;
        if go_up {
            self.up(&ctx);
        } else if let Some(at) = double.or(self.selection.cursor.filter(|_| activate)) {
            if self.entries[at].directory {
                self.open(&ctx, self.entries[at].path.clone());
            } else {
                if self.mode == Mode::Save {
                    self.name = self.entries[at].name.clone();
                }
                if self.selection.files.is_empty() {
                    self.selection.click(at, false, false, &self.entries);
                }
                self.pick(&ctx);
            }
        } else if activate {
            self.pick(&ctx);
        }
    }

    fn up(&mut self, ctx: &Context) {
        if let Some(parent) = self.browsing.as_ref().unwrap_or(&self.directory).parent() {
            self.open(ctx, parent.to_owned());
        }
    }

    fn open(&mut self, ctx: &Context, path: PathBuf) {
        self.path = path.display().to_string();
        self.browsing = Some(path.clone());
        self.entries.clear();
        self.row_ids.clear();
        self.selection = Selection::default();
        self.capped = false;
        self.request(ctx, Request::Open(path));
    }

    fn pick(&mut self, ctx: &Context) {
        if self.mode == Mode::Save {
            match save_path(&self.directory, &self.name) {
                Ok(path) => self.request(ctx, Request::Save(path)),
                Err(error) => self.error = Some(error),
            }
            return;
        }
        let paths = self.selection.paths(&self.entries);
        if !paths.is_empty() {
            self.request(ctx, Request::Pick(paths));
        }
    }

    fn request(&mut self, ctx: &Context, request: Request) {
        self.error = None;
        self.pending = None;
        if WORKERS.fetch_add(1, Ordering::SeqCst) >= WORKER_LIMIT {
            WORKERS.fetch_sub(1, Ordering::SeqCst);
            self.error = Some(own("file-chooser-workers-busy"));
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let hidden = self.hidden;
        let spawned = std::thread::Builder::new()
            .name("file-chooser".into())
            .spawn(move || {
                let answer = match request {
                    Request::Open(path) => open_path(path, hidden),
                    Request::Pick(paths) => check_files(paths).map(Answer::Picked),
                    Request::Save(path) => check_save(path),
                };
                let _ = tx.send(answer);
                WORKERS.fetch_sub(1, Ordering::SeqCst);
                ctx.request_repaint();
            });
        match spawned {
            Ok(_) => self.pending = Some(rx),
            Err(error) => {
                WORKERS.fetch_sub(1, Ordering::SeqCst);
                self.error = Some(error.to_string());
            }
        }
    }
}

fn save_path(directory: &Path, name: &str) -> Result<PathBuf, String> {
    if name.trim().is_empty() {
        return Err(own("file-chooser-name-required"));
    }
    if name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(own("file-chooser-name-invalid"));
    }
    Ok(directory.join(name))
}

fn check_save(path: PathBuf) -> Result<Answer, String> {
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(own_with(
            "file-chooser-name-link",
            &[(
                "name",
                &path.file_name().unwrap_or_default().to_string_lossy(),
            )],
        )),
        Ok(meta) if meta.is_file() => Ok(Answer::Save(path, true)),
        Ok(_) => Err(own("file-chooser-name-invalid")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Answer::Save(path, false)),
        Err(error) => Err(error.to_string()),
    }
}

/// Resolve XDG's Documents folder on a worker, since user-dirs.dirs is a file.
pub fn documents() -> PathBuf {
    let home = home();
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).ok();
    let env = std::env::var("XDG_DOCUMENTS_DIR").ok();
    documents_dir(env.as_deref(), text.as_deref(), &home)
}

fn documents_dir(env: Option<&str>, user_dirs: Option<&str>, home: &Path) -> PathBuf {
    user_directory(env, user_dirs, "XDG_DOCUMENTS_DIR", home).unwrap_or_else(|| {
        let documents = home.join("Documents");
        if documents.is_dir() {
            documents
        } else {
            home.to_owned()
        }
    })
}

/// Resolve an XDG user directory from the environment or user-dirs.dirs.
/// Filesystem access and the caller's default belong on its worker.
pub fn user_directory(
    env: Option<&str>,
    user_dirs: Option<&str>,
    key: &str,
    home: &Path,
) -> Option<PathBuf> {
    let expand = |value: &str| {
        let value = value.trim().trim_matches('"');
        let path = if value == "$HOME" {
            home.to_owned()
        } else if let Some(relative) = value.strip_prefix("$HOME/") {
            home.join(relative)
        } else {
            PathBuf::from(value)
        };
        path.is_absolute().then_some(path)
    };
    if let Some(path) = env.and_then(expand) {
        return Some(path);
    }
    if let Some(text) = user_dirs {
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix(&format!("{key}="))
                && let Some(path) = expand(value)
            {
                return Some(path);
            }
        }
    }
    None
}

/// The user's home; no stat is needed on the UI thread.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn resolve_path(directory: &Path, text: &str, home: &Path) -> PathBuf {
    if text == "~" {
        home.to_owned()
    } else if let Some(relative) = text.strip_prefix("~/") {
        home.join(relative)
    } else {
        directory.join(text)
    }
}

fn check_files(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, String> {
    for path in &paths {
        let metadata = std::fs::metadata(path).map_err(|error| {
            own_with(
                "file-chooser-path-error",
                &[
                    ("path", &path.display().to_string()),
                    ("error", &error.to_string()),
                ],
            )
        })?;
        if !path.is_absolute() || !metadata.is_file() {
            return Err(own_with(
                "file-chooser-not-file",
                &[("path", &path.display().to_string())],
            ));
        }
    }
    Ok(paths)
}

fn open_path(path: PathBuf, hidden: bool) -> Result<Answer, String> {
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let metadata = std::fs::metadata(&path).map_err(|error| {
        own_with(
            "file-chooser-path-error",
            &[
                ("path", &path.display().to_string()),
                ("error", &error.to_string()),
            ],
        )
    })?;
    if metadata.is_dir() {
        list_directory(&path, hidden).map(Answer::Listed)
    } else {
        check_files(vec![path]).map(Answer::Picked)
    }
}

fn visible(name: &str, hidden: bool) -> bool {
    hidden || !name.starts_with('.')
}

fn sorted_entries(entries: impl Iterator<Item = Entry>) -> (Vec<Entry>, bool) {
    let mut entries: Vec<_> = entries.take(ENTRY_LIMIT + 1).collect();
    let capped = entries.len() > ENTRY_LIMIT;
    entries.truncate(ENTRY_LIMIT);
    entries.sort_by_cached_key(|entry| {
        (
            !entry.directory,
            entry.name.to_lowercase(),
            entry.name.clone(),
        )
    });
    (entries, capped)
}

fn list_directory(path: &Path, hidden: bool) -> Result<Listing, String> {
    let items = std::fs::read_dir(path).map_err(|e| e.to_string())?;
    let entries = items.filter_map(|item| {
        let item = item.ok()?;
        let name = item.file_name().to_string_lossy().into_owned();
        if !visible(&name, hidden) {
            return None;
        }
        let metadata = std::fs::metadata(item.path()).ok()?;
        if !metadata.is_dir() && !metadata.is_file() {
            return None;
        }
        Some(Entry {
            path: item.path(),
            name,
            directory: metadata.is_dir(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        })
    });
    let (entries, capped) = sorted_entries(entries);
    Ok(Listing {
        directory: path.to_owned(),
        entries,
        capped,
    })
}

fn modified_date(time: SystemTime) -> Option<String> {
    // Gregorian civil date from whole UTC days since the Unix epoch.
    let seconds = match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).ok()?,
        Err(error) => {
            -i64::try_from(error.duration().as_secs()).ok()?
                - i64::from(error.duration().subsec_nanos() != 0)
        }
    };
    let days = seconds.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_name_and_folder_separators_have_distinct_messages() {
        for name in ["", " "] {
            assert_eq!(
                save_path(Path::new("/tmp"), name).unwrap_err(),
                "Enter a file name."
            );
        }
        for name in ["sub/mail.md", "mail\0.md", ".", ".."] {
            assert_eq!(
                save_path(Path::new("/tmp"), name).unwrap_err(),
                "Enter a file name without a folder separator."
            );
        }
    }

    #[test]
    fn documents_resolves_xdg_then_existing_documents_then_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        assert_eq!(documents_dir(None, None, home), home);
        std::fs::create_dir(home.join("Documents")).unwrap();
        assert_eq!(documents_dir(None, None, home), home.join("Documents"));
        let config = "XDG_DOCUMENTS_DIR=\"$HOME/Papers\"\n";
        assert_eq!(documents_dir(None, Some(config), home), home.join("Papers"));
        assert_eq!(
            documents_dir(Some("\"$HOME/Letters\""), Some(config), home),
            home.join("Letters")
        );
        assert_eq!(
            documents_dir(Some("/tmp/letters"), Some(config), home),
            Path::new("/tmp/letters")
        );
        assert_eq!(
            documents_dir(Some("relative"), None, home),
            home.join("Documents")
        );
        assert_eq!(
            documents_dir(None, Some("XDG_DOCUMENTS_DIR=\"$HOME\""), home),
            home
        );
    }

    #[test]
    fn save_opens_with_filename_focus_and_typing_then_enter_confirms() {
        use egui_kittest::kittest::Queryable;
        let dir = tempfile::tempdir().unwrap();
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, state: &mut (FileChooser, Option<Outcome>)| {
                    if state.1.is_none() {
                        state.1 = state.0.show(ui.ctx());
                    }
                },
                (
                    FileChooser::save(dir.path(), "Save document", "Write"),
                    None,
                ),
            );
        crate::install(&h.ctx, &crate::Theme::embedded());
        h.run_steps(5);
        assert!(h.get_by_label("File name").is_focused());
        h.event(egui::Event::Text("typed.md".into()));
        h.run();
        assert_eq!(h.state().0.name, "typed.md");
        // Let the real directory worker finish before submitting.
        for _ in 0..100 {
            if h.state().0.pending.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            h.run();
        }
        assert!(h.state().0.pending.is_none());
        h.key_press(Key::Enter);
        h.run();
        for _ in 0..100 {
            if h.state().1.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            h.run();
        }
        assert_eq!(
            h.state().1,
            Some(Outcome::Save {
                path: dir.path().join("typed.md"),
                replace: false
            })
        );
    }

    fn entry(name: &str, directory: bool) -> Entry {
        Entry {
            path: Path::new("/tmp").join(name),
            name: name.into(),
            directory,
            size: 0,
            modified: None,
        }
    }

    #[test]
    fn save_refuses_file_directory_and_dangling_links_before_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mail.md");
        std::fs::write(&file, "keep").unwrap();
        for target in [
            file.clone(),
            dir.path().to_owned(),
            dir.path().join("missing"),
        ] {
            let link = dir.path().join("link.md");
            std::os::unix::fs::symlink(target, &link).unwrap();
            assert_eq!(
                check_save(link.clone()).err().as_deref(),
                Some("link.md is a link; choose another name")
            );
            std::fs::remove_file(link).unwrap();
        }
        assert_eq!(std::fs::read_to_string(file).unwrap(), "keep");
    }

    #[test]
    fn save_mode_requires_a_name_joins_paths_and_checks_replacements() {
        let dir = tempfile::tempdir().unwrap();
        let mut chooser = FileChooser::save(dir.path(), "Save document", "Write");
        assert!(!chooser.can_confirm());
        for name in ["", " ", ".", "..", "/tmp/mail.md", "sub/mail.md"] {
            assert!(save_path(dir.path(), name).is_err());
        }
        chooser.name = "mail.md".into();
        assert!(chooser.can_confirm());
        let path = save_path(dir.path(), &chooser.name).unwrap();
        assert_eq!(path, dir.path().join("mail.md"));
        assert!(matches!(
            check_save(path.clone()),
            Ok(Answer::Save(_, false))
        ));
        std::fs::write(&path, "old").unwrap();
        assert!(matches!(check_save(path), Ok(Answer::Save(_, true))));
        assert!(check_save(dir.path().to_owned()).is_err());
    }

    fn save_fixture() -> FileChooser {
        let mut chooser =
            FileChooser::save("/home/user/Documents", "Save mail document as", "Save");
        chooser.started = true;
        chooser.name = "message.md".into();
        chooser.entries = vec![entry("Reports", true), entry("notes.md", false)];
        chooser
    }

    #[test]
    fn file_chooser_save_snapshot() {
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, chooser: &mut FileChooser| {
                    chooser.show(ui.ctx());
                },
                save_fixture(),
            );
        crate::install(&h.ctx, &crate::Theme::embedded());
        h.run_steps(5);
        h.snapshot("file_chooser_save");
    }

    #[test]
    fn save_existing_file_waits_for_replace_and_cancel_keeps_chooser() {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, state: &mut (FileChooser, Option<Outcome>)| {
                    if state.1.is_none() {
                        state.1 = state.0.show(ui.ctx());
                    }
                },
                (save_fixture(), None),
            );
        crate::install(&h.ctx, &crate::Theme::embedded());
        h.run_steps(5);
        let path = PathBuf::from("/home/user/Documents/message.md");
        let (tx, rx) = mpsc::channel();
        h.state_mut().0.pending = Some(rx);
        tx.send(Ok(Answer::Save(path.clone(), true))).unwrap();
        h.run();
        assert!(h.state().1.is_none());
        assert!(h.state().0.replace.is_some());
        h.key_press(Key::Escape);
        h.run();
        assert!(h.state().0.replace.is_none());
        assert!(h.state().1.is_none());
        let (tx, rx) = mpsc::channel();
        h.state_mut().0.pending = Some(rx);
        tx.send(Ok(Answer::Save(path.clone(), true))).unwrap();
        h.run();
        assert!(h.query_by_label("Replace").is_some());
        h.get_by_label("Replace").focus();
        h.run();
        assert!(
            h.get_by_label("Replace").is_focused(),
            "replacement button must own focus"
        );
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(
            h.state().1,
            Some(Outcome::Save {
                path,
                replace: true
            })
        );
    }

    #[test]
    fn folders_first_then_names_ignoring_case_and_bounded() {
        let (entries, capped) = sorted_entries(
            vec![
                entry("z", true),
                entry("a", false),
                entry("B", false),
                entry("A", true),
            ]
            .into_iter(),
        );
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["A", "z", "a", "B"]
        );
        assert!(!capped);
        let (entries, capped) =
            sorted_entries((0..ENTRY_LIMIT + 20).map(|i| entry(&i.to_string(), false)));
        assert_eq!(entries.len(), ENTRY_LIMIT);
        assert!(capped);
        assert!(!sorted_entries((0..ENTRY_LIMIT).map(|i| entry(&i.to_string(), false))).1);
    }

    #[test]
    fn hidden_filter_and_path_resolution() {
        assert!(!visible(".hidden", false));
        assert!(visible(".hidden", true));
        assert!(visible("file", false));
        let directory = Path::new("/tmp/files");
        let home = Path::new("/home/user");
        assert_eq!(resolve_path(directory, "~", home), home);
        assert_eq!(resolve_path(directory, "~/docs", home), home.join("docs"));
        assert_eq!(
            resolve_path(directory, "note.txt", home),
            directory.join("note.txt")
        );
        assert_eq!(resolve_path(directory, "/tmp/a", home), Path::new("/tmp/a"));
    }

    #[test]
    fn selection_toggles_ranges_and_all_never_pick_folders() {
        let entries = vec![
            entry("folder", true),
            entry("a", false),
            entry("b", false),
            entry("c", false),
        ];
        let mut selection = Selection::default();
        selection.click(1, false, false, &entries);
        selection.click(3, true, false, &entries);
        assert_eq!(
            selection.paths(&entries),
            [entries[1].path.clone(), entries[3].path.clone()]
        );
        selection.click(3, true, false, &entries);
        assert_eq!(selection.files, BTreeSet::from([1]));
        selection.click(1, false, true, &entries);
        assert_eq!(selection.files, BTreeSet::from([1, 2, 3]));
        selection.click(0, false, true, &entries);
        assert_eq!(selection.files, BTreeSet::from([1, 2, 3]));
        selection.click(0, false, false, &entries);
        assert!(selection.files.is_empty());
        selection.all(&entries);
        assert_eq!(selection.files, BTreeSet::from([1, 2, 3]));
    }

    #[test]
    fn symlinks_followed_but_directories_and_devices_cannot_be_picked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        std::fs::write(&path, b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(check_files(vec![link.clone()]).unwrap(), [link]);
        assert!(check_files(vec![dir.path().to_owned()]).is_err());
        assert!(check_files(vec!["/dev/null".into()]).is_err());
        assert_eq!(modified_date(UNIX_EPOCH).as_deref(), Some("1970-01-01"));
        assert_eq!(
            modified_date(UNIX_EPOCH - std::time::Duration::from_secs(1)).as_deref(),
            Some("1969-12-31")
        );
        assert_eq!(
            modified_date(UNIX_EPOCH + std::time::Duration::from_secs(951_782_400)).as_deref(),
            Some("2000-02-29")
        );
    }

    #[test]
    fn listing_keeps_text_while_the_path_field_has_focus() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        let mut chooser = FileChooser::new("/tmp", "Choose files", "Attach");
        chooser.started = true;
        let mut h = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, chooser: &mut FileChooser| {
                    chooser.show(ui.ctx());
                },
                chooser,
            );
        crate::install(&h.ctx, &crate::Theme::embedded());
        h.run_steps(5);
        h.get_by_label("Path").focus();
        h.run();
        assert!(h.get_by_label("Path").is_focused());
        h.state_mut().path = "/tmp/still typing".into();
        let (tx, rx) = mpsc::channel();
        h.state_mut().pending = Some(rx);
        tx.send(Ok(Answer::Listed(Listing {
            directory: "/tmp/listed".into(),
            entries: vec![entry("report.txt", false)],
            capped: false,
        })))
        .unwrap();
        h.run();
        assert_eq!(h.state().directory, PathBuf::from("/tmp/listed"));
        assert_eq!(h.state().path, "/tmp/still typing");
        assert_eq!(h.state().entries.len(), 1);

        h.ctx
            .memory_mut(|m| m.surrender_focus(h.state().id.with("path")));
        let (tx, rx) = mpsc::channel();
        h.state_mut().pending = Some(rx);
        tx.send(Ok(Answer::Listed(Listing {
            directory: "/tmp/next".into(),
            entries: Vec::new(),
            capped: false,
        })))
        .unwrap();
        h.run();
        assert_eq!(h.state().path, "/tmp/next");
    }

    #[test]
    fn picking_clears_the_dialog_placement() {
        use egui_kittest::Harness;

        let mut chooser = FileChooser::new("/tmp", "Choose files", "Attach");
        chooser.started = true;
        let mut h = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, state: &mut (FileChooser, Option<Outcome>)| {
                    if state.1.is_none() {
                        let before = ui.ctx().data(|data| data.len());
                        state.1 = state.0.show(ui.ctx());
                        if matches!(state.1, Some(Outcome::Picked(_))) {
                            // Pick returns before drawing: only the placement
                            // entry should disappear during this call.
                            assert_eq!(ui.ctx().data(|data| data.len()) + 1, before);
                        }
                    }
                },
                (chooser, None),
            );
        crate::install(&h.ctx, &crate::Theme::embedded());
        h.run_steps(5);
        let (tx, rx) = mpsc::channel();
        h.state_mut().0.pending = Some(rx);
        let paths = vec![PathBuf::from("/tmp/report.txt")];
        tx.send(Ok(Answer::Picked(paths.clone()))).unwrap();
        h.run();
        assert_eq!(h.state().1, Some(Outcome::Picked(paths)));
    }

    #[test]
    fn enter_on_a_focused_cancel_button_cancels_instead_of_picking() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        let mut chooser = FileChooser::new("/tmp", "Choose files", "Attach");
        chooser.started = true;
        chooser.entries = vec![entry("report.txt", false)];
        chooser.selection.all(&chooser.entries);
        let mut h = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, state: &mut (FileChooser, Option<Outcome>)| {
                    if state.1.is_none() {
                        state.1 = state.0.show(ui.ctx());
                    }
                },
                (chooser, None),
            );
        crate::install(&h.ctx, &crate::Theme::embedded());
        h.run_steps(5);
        h.get_by_label("Cancel").focus();
        h.run();
        assert!(h.get_by_label("Cancel").is_focused());
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(h.state().1, Some(Outcome::Cancelled));
        assert!(h.state().0.pending.is_none(), "no pick was started");
    }

    #[test]
    fn stacked_chooser_exposes_its_widgets_and_escape_only_cancels_it() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;

        let mut chooser = FileChooser::new("/tmp", "Choose files", "Attach");
        chooser.started = true;
        chooser.entries = vec![entry("report.txt", false)];
        let mut h = Harness::builder()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, state: &mut (FileChooser, bool, Option<Outcome>)| {
                    let parent = Dialog::new(Id::unique("parent-compose"), "Compose")
                        .enabled(false)
                        .buttons(vec![Choice::new("Close compose", Role::Cancel)])
                        .show(ui.ctx(), |ui| {
                            ui.label("Message body");
                        });
                    state.1 |= parent.chosen.is_some() || parent.dismissed;
                    if state.2.is_none() {
                        state.2 = state.0.show(ui.ctx());
                    }
                },
                (chooser, false, None),
            );
        crate::install(&h.ctx, &crate::Theme::embedded());
        h.run_steps(5);
        let _ = h.get_by_label("Path");
        let _ = h.get_by_label("report.txt");
        let _ = h.get_by_label("Attach");
        let _ = h.get_by_label("Cancel");
        h.key_press(Key::Escape);
        h.run();
        assert_eq!(h.state().2, Some(Outcome::Cancelled));
        assert!(!h.state().1);
    }
}
