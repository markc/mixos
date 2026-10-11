// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's state around the controller: what the window shows beyond
//! the documents (view options, the find bar, panels, dialogs, notices), and
//! the routing of everything the controller asks of a window. No drawing
//! happens here, so it is tested without a display.
//!
//! - Every action goes through [`App::action`] (menus, shortcuts, the tab
//!   strip) or the Bus (`ced.action`), and reaches the controller the same
//!   way.
//! - The controller's window-only actions (dialogs, the find bar, view
//!   options, zoom, panels, help) are carried out here. Each is answered
//!   exactly once, when the window has acted: a cancellation (Esc, its tab
//!   closing, quitting) or a view change through [`Controller::ui_done`],
//!   and a dialog's accepted answer through [`Controller::ui_run`], which
//!   runs the action with it and answers as that action does when called
//!   with the arguments directly: a Save As when the save completes or
//!   fails, an Open with its new tabs, a Goto with the position asked for
//!   (one outside the document leaves the caret where it was, as the
//!   direct call does).
//! - Prompts become dialogs. A prompt that arrives while a dialog is open
//!   waits behind it.
//! - Notices: information shows briefly in the status bar; warnings and
//!   errors stay in the notice strip until dismissed (the last four).
//! - Effects that need the Bus, the clipboard, a worker thread or the
//!   process ([`App::effects`]) are left for the shell.

use std::collections::VecDeque;
use std::path::PathBuf;

use documents::actions::ActionId;
use documents::config::{self, Config, FONT_PX_MAX, FONT_PX_MIN};
use documents::controller::{Controller, Effect, Prompt, RecoveredRow, Tab};
use documents::editor::{EditorMsg, LayoutReport};
use documents::verbs::{Refusal, code};
use editor::{View, Wrap};
use editor_model::mirror::Phase;
use editor_model::types::{Intent, Level, Notice, TabId};
use serde::Serialize;
use serde_json::{Value, json};

/// The Mono size when nothing sets it, in points.
pub const DEFAULT_FONT_PX: u16 = 13;
/// How long an information notice stays in the status bar, in seconds.
pub const STATUS_SECONDS: f64 = 6.0;
/// Warnings and errors kept in the notice strip.
const NOTICES_MAX: usize = 4;

/// The find bar.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FindBar {
    pub pattern: String,
    pub replacement: String,
    pub regex: bool,
    /// Case-sensitive.
    pub case: bool,
    /// Show the replace row.
    pub replace: bool,
}

/// What the window shows besides the documents: one serialisable struct.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiState {
    /// Text size in points.
    pub font_px: u16,
    pub whitespace: bool,
    pub line_numbers: bool,
    pub remote_carets: bool,
    pub problems: bool,
    pub output: bool,
    pub find: Option<FindBar>,
}

/// A modal dialog. Those opened by a window-only action carry its `token`
/// and the intent of whoever asked (the person, or a Bus caller).
#[derive(Debug, Clone, PartialEq)]
pub enum Dialog {
    Open {
        path: String,
        token: u64,
        intent: Intent,
    },
    SaveAs {
        tab: TabId,
        path: String,
        token: u64,
        intent: Intent,
    },
    Goto {
        tab: TabId,
        text: String,
        token: u64,
        intent: Intent,
    },
    CloseDirty {
        tab: TabId,
        intent: Intent,
    },
    DiskModified {
        tab: TabId,
        intent: Intent,
    },
    Recovered {
        rows: Vec<RecoveredRow>,
    },
    Keys {
        token: u64,
    },
    About {
        token: u64,
    },
}

impl Dialog {
    /// The window-only action this dialog answers, if any.
    fn token(&self) -> Option<u64> {
        match self {
            Dialog::Open { token, .. }
            | Dialog::SaveAs { token, .. }
            | Dialog::Goto { token, .. }
            | Dialog::Keys { token }
            | Dialog::About { token } => Some(*token),
            _ => None,
        }
    }

    /// The document this dialog is about, if any.
    fn tab(&self) -> Option<TabId> {
        match self {
            Dialog::SaveAs { tab, .. }
            | Dialog::Goto { tab, .. }
            | Dialog::CloseDirty { tab, .. }
            | Dialog::DiskModified { tab, .. } => Some(*tab),
            _ => None,
        }
    }
}

/// A warning or error in the notice strip.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub tab: Option<TabId>,
    pub level: Level,
    pub text: String,
}

/// What the person did in a dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The default button (Open, Save, Go…).
    Accept,
    /// The alternate button (Don't Save).
    Other,
    Cancel,
    /// A recovered document: open it (`true`) or discard it.
    Recovered {
        buffer: String,
        open: bool,
    },
}

pub struct App {
    pub ctl: Controller,
    pub config: Config,
    /// Where the configuration was read from (View › Reload Settings).
    pub config_path: Option<PathBuf>,
    pub ui: UiState,
    pub dialog: Option<Dialog>,
    /// Prompts waiting for the open dialog to close.
    queued: VecDeque<Dialog>,
    pub notices: VecDeque<Shown>,
    /// A transient status line and when it expires.
    pub status: Option<(String, f64)>,
    /// Effects for the shell: Bus work, clipboard, relex, session, quit.
    pub effects: Vec<Effect>,
    /// The clock the status expiry is measured against (the frame time).
    pub now: f64,
    /// This window's own theme choice (View › Theme).
    pub theme: toolkit::theme_menu::Choice,
    /// The shown editor's widget id, as last drawn: the shortcuts apply
    /// unless some other widget has the keyboard.
    pub editor_id: Option<egui::Id>,
    /// The editor holds input for its next frame: shortcuts wait for it.
    pub editor_held: bool,
    /// Give the find field keyboard focus on the next frame.
    pub focus_find: bool,
}

impl App {
    pub fn new(ctl: Controller, config: Config) -> Self {
        let ui = Self::ui_from(&config, false, false, None);
        Self {
            ctl,
            config,
            config_path: None,
            ui,
            dialog: None,
            queued: VecDeque::new(),
            notices: VecDeque::new(),
            status: None,
            effects: Vec::new(),
            now: 0.0,
            theme: toolkit::theme_menu::Choice::default(),
            editor_id: None,
            editor_held: false,
            focus_find: false,
        }
    }

    fn ui_from(config: &Config, problems: bool, output: bool, find: Option<FindBar>) -> UiState {
        UiState {
            font_px: config.font_px.unwrap_or(DEFAULT_FONT_PX),
            whitespace: config.show_whitespace,
            line_numbers: config.line_numbers,
            remote_carets: config.remote_carets,
            problems,
            output,
            find,
        }
    }

    pub fn active(&self) -> Option<TabId> {
        self.ctl.active()
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        let id = self.ctl.active()?;
        self.tab(id)
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.ctl.tabs().iter().find(|t| t.id == id)
    }

    fn intent(&self) -> Intent {
        Intent::ui(self.active().unwrap_or(0))
    }

    /// Run `action` as the person did (a menu, a shortcut).
    pub fn action(&mut self, action: ActionId) {
        let fx = self.ctl.on_action(self.active(), action, self.intent());
        self.absorb(fx);
    }

    /// Run `action` with its arguments, as the person did (the find bar).
    pub fn action_args(&mut self, tab: Option<TabId>, action: ActionId, args: Value) {
        let fx = self
            .ctl
            .on_action_args(tab, action, Some(args), self.intent());
        self.absorb(fx);
    }

    /// Whether `action` can run now (menus grey it out otherwise), as the
    /// controller would allow it.
    pub fn enabled(&self, action: ActionId) -> bool {
        use ActionId::*;
        if self.dialog.is_some() {
            return false;
        }
        let mirror = self.active_tab().and_then(|t| t.mirror.as_ref());
        let live = mirror.is_some_and(|m| matches!(m.phase(), Phase::Live));
        let tab = self.active().is_some();
        match action {
            FileNew | FileOpen | FileExit | FileSaveAll | HelpKeys | HelpAbout | ViewZoomIn
            | ViewZoomOut | ViewZoomReset | ViewWhitespace | ViewLineNumbers | ViewRemoteCarets
            | ViewProblems | ViewOutput | ViewReloadSettings => true,
            FileClose | ViewClearMarkers => tab,
            // Text can be copied and selected out of a detached document.
            EditCopy | EditSelectAll => mirror.is_some(),
            EditUndoOther => live && mirror.is_some_and(|m| m.last_remote().is_some()),
            TabsNext | TabsPrev => self.ctl.tabs().len() > 1,
            TabsGoto(n) => usize::from(n) <= self.ctl.tabs().len(),
            _ => live,
        }
    }

    /// Whether a toggle action is on (menus show a tick).
    pub fn checked(&self, action: ActionId) -> Option<bool> {
        use ActionId::*;
        match action {
            ViewWhitespace => Some(self.ui.whitespace),
            ViewLineNumbers => Some(self.ui.line_numbers),
            ViewRemoteCarets => Some(self.ui.remote_carets),
            ViewProblems => Some(self.ui.problems),
            ViewOutput => Some(self.ui.output),
            EditOverwrite => Some(self.active_tab().is_some_and(|t| t.editor.overwrite)),
            _ => None,
        }
    }

    /// Take what the controller asked for: dialogs, window-only actions and
    /// notices here, everything else for the shell.
    pub fn absorb(&mut self, fx: Vec<Effect>) {
        let mut work: VecDeque<Effect> = fx.into();
        while let Some(effect) = work.pop_front() {
            match effect {
                Effect::Prompt(prompt) => self.prompt(prompt),
                Effect::UiAction {
                    tab,
                    action,
                    args,
                    intent,
                    token,
                } => {
                    let more = self.window_action(tab, action, args, intent, token);
                    work.extend(more);
                }
                Effect::Notice { tab, notice } => self.notice(tab, notice),
                other => self.effects.push(other),
            }
        }
    }

    fn notice(&mut self, tab: Option<TabId>, notice: Notice) {
        // Conflicts and detached copies are drawn from the mirror's state.
        let Notice::Message { level, text } = notice else {
            return;
        };
        if level == Level::Info {
            self.status = Some((text, self.now + STATUS_SECONDS));
            return;
        }
        self.notices.push_back(Shown { tab, level, text });
        while self.notices.len() > NOTICES_MAX {
            self.notices.pop_front();
        }
    }

    /// The status line, while it lasts.
    pub fn status(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|(_, until)| *until > self.now)
            .map(|(text, _)| text.as_str())
    }

    pub fn dismiss_notice(&mut self, index: usize) {
        if index < self.notices.len() {
            self.notices.remove(index);
        }
    }

    fn prompt(&mut self, prompt: Prompt) {
        let dialog = match prompt {
            Prompt::CloseDirty { tab, intent } => Dialog::CloseDirty { tab, intent },
            Prompt::DiskModified { tab, intent } => Dialog::DiskModified { tab, intent },
            Prompt::Recovered { buffers } => Dialog::Recovered { rows: buffers },
        };
        self.show(dialog);
    }

    fn show(&mut self, dialog: Dialog) {
        if self.dialog.is_some() {
            self.queued.push_back(dialog);
        } else {
            self.dialog = Some(dialog);
        }
    }

    fn tab_exists(&self, tab: TabId) -> bool {
        self.tab(tab).is_some()
    }

    /// Close the dialog and show the next queued one. A queued dialog whose
    /// tab has gone is cancelled.
    fn next_dialog(&mut self) {
        self.dialog = None;
        while let Some(next) = self.queued.pop_front() {
            if next.tab().is_none_or(|t| self.tab_exists(t)) {
                self.dialog = Some(next);
                return;
            }
            self.cancel(&next);
        }
    }

    /// Answer a dialog's window-only action as cancelled.
    fn cancel(&mut self, dialog: &Dialog) {
        if let Some(token) = dialog.token() {
            let fx = self.done(token, Err(Self::refused(code::CONFLICT, "cancelled")));
            self.absorb(fx);
        }
    }

    /// Cancel the open dialog if its document has gone (each frame).
    pub fn prune(&mut self) {
        if let Some(dialog) = self.dialog.clone()
            && dialog.tab().is_some_and(|t| !self.tab_exists(t))
        {
            self.cancel(&dialog);
            self.next_dialog();
        }
    }

    /// Quitting: cancel the open and the queued dialogs' actions.
    pub fn cancel_dialogs(&mut self) {
        let all: Vec<Dialog> = self
            .dialog
            .take()
            .into_iter()
            .chain(std::mem::take(&mut self.queued))
            .collect();
        for dialog in all {
            self.cancel(&dialog);
        }
    }

    fn done(&mut self, token: u64, result: Result<Value, Refusal>) -> Vec<Effect> {
        self.ctl.ui_done(token, result)
    }

    fn refused(error_code: &str, reason: &str) -> Refusal {
        let message = match reason {
            "cancelled" => "cancelled in the window",
            "no_document" => "no document is open",
            _ => "the answer was not valid",
        };
        Refusal {
            error_code: error_code.into(),
            message: message.into(),
            reason: Some(reason.into()),
        }
    }

    /// A window-only action: the window acts, then answers `token`.
    fn window_action(
        &mut self,
        tab: Option<TabId>,
        action: ActionId,
        args: Option<Value>,
        intent: Intent,
        token: u64,
    ) -> Vec<Effect> {
        use ActionId::*;
        let arg = |k: &str| args.as_ref().and_then(|a| a.get(k)).cloned();
        let ok = || Ok(json!({}));
        match action {
            ViewZoomIn | ViewZoomOut | ViewZoomReset => {
                self.ui.font_px = match action {
                    ViewZoomIn => (self.ui.font_px + 1).min(FONT_PX_MAX),
                    ViewZoomOut => self.ui.font_px.saturating_sub(1).max(FONT_PX_MIN),
                    _ => self.config.font_px.unwrap_or(DEFAULT_FONT_PX),
                };
                self.done(token, Ok(json!({ "font_px": self.ui.font_px })))
            }
            ViewWhitespace => {
                self.ui.whitespace = !self.ui.whitespace;
                self.done(token, ok())
            }
            ViewLineNumbers => {
                self.ui.line_numbers = !self.ui.line_numbers;
                self.done(token, ok())
            }
            ViewRemoteCarets => {
                self.ui.remote_carets = !self.ui.remote_carets;
                self.done(token, ok())
            }
            ViewProblems => {
                self.ui.problems = !self.ui.problems;
                self.done(token, ok())
            }
            ViewOutput => {
                self.ui.output = !self.ui.output;
                self.done(token, ok())
            }
            ViewReloadSettings => {
                let result = self.reload_settings();
                self.done(token, result)
            }
            SearchFind | SearchReplace | SearchFindNext | SearchFindPrev | SearchReplaceAll => {
                // An open dialog keeps the keyboard (a Bus caller can open
                // the bar under it).
                if self.dialog.is_none()
                    && (self.ui.find.is_none() || matches!(action, SearchFind | SearchReplace))
                {
                    self.focus_find = true;
                }
                let bar = self.ui.find.get_or_insert_with(FindBar::default);
                if let Some(p) = arg("pattern").and_then(|v| v.as_str().map(str::to_owned)) {
                    bar.pattern = p;
                }
                if matches!(action, SearchReplace | SearchReplaceAll) {
                    bar.replace = true;
                }
                if !bar.pattern.is_empty() && !matches!(action, SearchFind | SearchReplace) {
                    // The bar already holds a pattern: run the search with it.
                    let args = self.find_args();
                    let mut fx = self.ctl.on_action_args(tab, action, Some(args), intent);
                    fx.extend(self.done(token, ok()));
                    return fx;
                }
                self.done(token, ok())
            }
            FileOpen => {
                let path = self.directory_of_active();
                self.show(Dialog::Open {
                    path,
                    token,
                    intent,
                });
                Vec::new()
            }
            FileSaveAs => match tab.filter(|t| self.tab_exists(*t)) {
                Some(tab) => {
                    let path = self
                        .tab(tab)
                        .and_then(|t| t.path.clone())
                        .unwrap_or_else(|| self.directory_of_active());
                    self.show(Dialog::SaveAs {
                        tab,
                        path,
                        token,
                        intent,
                    });
                    Vec::new()
                }
                None => self.done(token, Err(Self::refused(code::NOT_FOUND, "no_document"))),
            },
            SearchGotoLine => match tab.filter(|t| self.tab_exists(*t)) {
                Some(tab) => {
                    self.show(Dialog::Goto {
                        tab,
                        text: String::new(),
                        token,
                        intent,
                    });
                    Vec::new()
                }
                None => self.done(token, Err(Self::refused(code::NOT_FOUND, "no_document"))),
            },
            HelpKeys => {
                self.show(Dialog::Keys { token });
                Vec::new()
            }
            HelpAbout => {
                self.show(Dialog::About { token });
                Vec::new()
            }
            _ => self.done(token, ok()),
        }
    }

    /// Read the configuration again and apply it to the window and the
    /// controller.
    fn reload_settings(&mut self) -> Result<Value, Refusal> {
        let Some(path) = self.config_path.clone() else {
            return Ok(json!({ "reloaded": false }));
        };
        let (config, note) = config::load(&path);
        if let Some(note) = note {
            self.notice(
                None,
                Notice::Message {
                    level: Level::Warn,
                    text: note,
                },
            );
        }
        let find = self.ui.find.take();
        self.ui = Self::ui_from(&config, self.ui.problems, self.ui.output, find);
        self.ctl.set_config(config.clone());
        self.config = config;
        Ok(json!({ "reloaded": true }))
    }

    /// The find bar as action arguments.
    pub fn find_args(&self) -> Value {
        let bar = self.ui.find.clone().unwrap_or_default();
        json!({
            "pattern": bar.pattern,
            "replacement": bar.replacement,
            "regex": bar.regex,
            "case": bar.case,
        })
    }

    /// Run a find-bar button.
    pub fn find(&mut self, action: ActionId) {
        if self.ui.find.as_ref().is_some_and(|b| !b.pattern.is_empty()) {
            let args = self.find_args();
            self.action_args(self.active(), action, args);
        }
    }

    pub fn close_find(&mut self) {
        self.ui.find = None;
    }

    /// Put set-aside text back at the caret. The conflict is forgotten only
    /// when the document can take the text now.
    pub fn reinsert(&mut self, tab: TabId, rev: u64, text: String) {
        let live = self
            .tab(tab)
            .and_then(|t| t.mirror.as_ref())
            .is_some_and(|m| matches!(m.phase(), Phase::Live));
        if !live {
            self.notice(
                Some(tab),
                Notice::Message {
                    level: Level::Warn,
                    text: crate::label("reinsert-later"),
                },
            );
            return;
        }
        let (took, fx) = self.ctl.insert(tab, text);
        if took {
            self.ctl.dismiss_conflict(tab, rev);
        }
        self.absorb(fx);
    }

    /// The active document's directory with a trailing slash, else the
    /// working directory's.
    fn directory_of_active(&self) -> String {
        let dir = self
            .active_tab()
            .and_then(|t| t.path.as_deref())
            .and_then(|p| std::path::Path::new(p).parent())
            .map(std::path::Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok());
        dir.map(|d| format!("{}/", d.display())).unwrap_or_default()
    }

    /// Edit a dialog's text field.
    pub fn dialog_text(&mut self) -> Option<&mut String> {
        match &mut self.dialog {
            Some(Dialog::Open { path, .. } | Dialog::SaveAs { path, .. }) => Some(path),
            Some(Dialog::Goto { text, .. }) => Some(text),
            _ => None,
        }
    }

    /// What the person chose in the open dialog.
    pub fn answer(&mut self, answer: Answer) {
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        let invalid = || Err(Self::refused(code::INVALID_ARGUMENT, "invalid"));
        let fx = match (dialog, answer) {
            (
                Dialog::Open {
                    path,
                    token,
                    intent,
                },
                Answer::Accept,
            ) => {
                let path = path.trim();
                if path.is_empty() || path.ends_with('/') {
                    // Keep the dialog open for a file name.
                    return;
                }
                self.ctl.ui_run(
                    token,
                    None,
                    ActionId::FileOpen,
                    json!({ "paths": [path] }),
                    intent,
                )
            }
            (
                Dialog::SaveAs {
                    tab,
                    path,
                    token,
                    intent,
                },
                Answer::Accept,
            ) => {
                let path = path.trim();
                if path.is_empty() || path.ends_with('/') {
                    return;
                }
                // The caller hears when the save completes or fails.
                self.ctl.ui_run(
                    token,
                    Some(tab),
                    ActionId::FileSaveAs,
                    json!({ "path": path }),
                    intent,
                )
            }
            (
                Dialog::Goto {
                    tab,
                    text,
                    token,
                    intent,
                },
                Answer::Accept,
            ) => match parse_line_col(&text) {
                Some((line, col)) => {
                    let mut args = json!({ "line": line });
                    if let Some(col) = col {
                        args["col"] = json!(col);
                    }
                    self.ctl
                        .ui_run(token, Some(tab), ActionId::SearchGotoLine, args, intent)
                }
                None if text.trim().is_empty() => return,
                None => self.done(token, invalid()),
            },
            (
                Dialog::Open { token, .. }
                | Dialog::SaveAs { token, .. }
                | Dialog::Goto { token, .. },
                _,
            ) => self.done(token, Err(Self::refused(code::CONFLICT, "cancelled"))),
            (Dialog::Keys { token } | Dialog::About { token }, _) => {
                self.done(token, Ok(json!({})))
            }
            (Dialog::CloseDirty { tab, intent }, Answer::Accept) => self.ctl.on_action_args(
                Some(tab),
                ActionId::FileClose,
                Some(json!({ "save": true })),
                intent,
            ),
            (Dialog::CloseDirty { tab, intent }, Answer::Other) => self.ctl.on_action_args(
                Some(tab),
                ActionId::FileClose,
                Some(json!({ "force": true })),
                intent,
            ),
            (Dialog::DiskModified { tab, intent }, Answer::Accept) => self.ctl.on_action_args(
                Some(tab),
                ActionId::FileSave,
                Some(json!({ "force": true })),
                intent,
            ),
            (Dialog::Recovered { mut rows }, Answer::Recovered { buffer, open }) => {
                rows.retain(|r| r.buffer != buffer);
                let intent = self.intent();
                let fx = if open {
                    self.ctl.open_recovered(&buffer, intent)
                } else {
                    self.ctl.discard_recovered(&buffer)
                };
                if !rows.is_empty() {
                    // The rest stay on screen.
                    self.dialog = Some(Dialog::Recovered { rows });
                    self.absorb(fx);
                    return;
                }
                fx
            }
            _ => Vec::new(),
        };
        self.next_dialog();
        self.absorb(fx);
    }

    /// The editor widget's event for `tab`, as the controller knows it.
    pub fn editor_event(&mut self, tab: TabId, event: editor::Event) {
        let msg = match event {
            editor::Event::Command(c) => EditorMsg::Command(c),
            editor::Event::Scrolled(s) => EditorMsg::Scrolled(s),
            editor::Event::Preedit(s) => EditorMsg::Preedit(s),
            editor::Event::Focus(f) => EditorMsg::Focus(f),
            editor::Event::Layout(r) => EditorMsg::Layout(LayoutReport {
                editor: r.editor,
                gutter_w: r.gutter_width,
                line_height: r.row_height,
                cell_w: r.cell_width,
                first_line: r.first_line,
                visible_rows: r.visible_rows,
                caret: r.caret,
            }),
            editor::Event::Undo => return self.action(ActionId::EditUndo),
            editor::Event::Redo => return self.action(ActionId::EditRedo),
        };
        let fx = self.ctl.on_editor(tab, msg);
        self.absorb(fx);
    }

    /// The editor's view options for `tab`: prose wraps, code scrolls.
    pub fn view_for(&self, tab: &Tab) -> View {
        let language = tab
            .mirror
            .as_ref()
            .map_or("text", |m| m.meta().language.as_str());
        let prose = matches!(language, "markdown" | "git_commit");
        View {
            font_size: Some(f32::from(self.ui.font_px)),
            tab_size: self.config.tab_size,
            ambiguous_wide: self.config.ambiguous_wide,
            wrap: if prose { Wrap::Words } else { Wrap::None },
            line_numbers: self.ui.line_numbers,
            whitespace: self.ui.whitespace,
            remote_carets: self.ui.remote_carets,
            matches: self.ctl.find_matches(tab.id).to_vec(),
            ..View::default()
        }
    }
}

/// `line` or `line:col`, both 1 or more.
fn parse_line_col(text: &str) -> Option<(usize, Option<usize>)> {
    let mut parts = text.trim().splitn(2, ':');
    let line = parts
        .next()?
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)?;
    let col = match parts.next() {
        Some(c) => Some(c.trim().parse::<usize>().ok().filter(|n| *n > 0)?),
        None => None,
    };
    Some((line, col))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(
            Controller::new(Config::default(), 1, false),
            Config::default(),
        )
    }

    #[test]
    fn notices_split_between_the_status_line_and_the_strip() {
        let mut a = app();
        a.now = 10.0;
        a.notice(
            None,
            Notice::Message {
                level: Level::Info,
                text: "Saved".into(),
            },
        );
        assert_eq!(a.status(), Some("Saved"));
        a.now = 10.0 + STATUS_SECONDS + 1.0;
        assert_eq!(a.status(), None, "information fades");
        for n in 0..6 {
            a.notice(
                Some(1),
                Notice::Message {
                    level: Level::Warn,
                    text: format!("w{n}"),
                },
            );
        }
        let texts: Vec<_> = a.notices.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["w2", "w3", "w4", "w5"], "the last four stay");
        a.dismiss_notice(0);
        assert_eq!(a.notices.len(), 3);
    }

    #[test]
    fn prompts_wait_behind_an_open_dialog() {
        let mut a = app();
        a.dialog = Some(Dialog::About { token: 0 });
        a.prompt(Prompt::Recovered {
            buffers: vec![RecoveredRow {
                buffer: "b1".into(),
                path: None,
                name: "untitled".into(),
                bytes: Some(3),
            }],
        });
        assert!(matches!(a.dialog, Some(Dialog::About { .. })));
        a.answer(Answer::Accept);
        assert!(matches!(a.dialog, Some(Dialog::Recovered { .. })));
    }

    #[test]
    fn dialogs_for_a_missing_document_are_never_shown() {
        let mut a = app();
        a.dialog = Some(Dialog::About { token: 0 });
        a.queued.push_back(Dialog::Goto {
            tab: 42,
            text: String::new(),
            token: 7,
            intent: Intent::ui(42),
        });
        a.answer(Answer::Accept);
        assert!(a.dialog.is_none(), "tab 42 does not exist: cancelled");
        a.dialog = Some(Dialog::SaveAs {
            tab: 9,
            path: "/x".into(),
            token: 8,
            intent: Intent::ui(9),
        });
        a.prune();
        assert!(
            a.dialog.is_none(),
            "an open dialog whose tab went is cancelled"
        );
    }

    #[test]
    fn quitting_cancels_every_dialog() {
        let mut a = app();
        a.dialog = Some(Dialog::Keys { token: 1 });
        a.queued.push_back(Dialog::About { token: 2 });
        a.cancel_dialogs();
        assert!(a.dialog.is_none() && a.queued.is_empty());
    }

    #[test]
    fn view_toggles_and_zoom_are_window_actions() {
        let mut a = app();
        let px = a.ui.font_px;
        let _ = a.window_action(None, ActionId::ViewZoomIn, None, Intent::ui(0), 7);
        assert_eq!(a.ui.font_px, px + 1);
        let _ = a.window_action(None, ActionId::ViewZoomReset, None, Intent::ui(0), 8);
        assert_eq!(a.ui.font_px, DEFAULT_FONT_PX);
        let ws = a.ui.whitespace;
        let _ = a.window_action(None, ActionId::ViewWhitespace, None, Intent::ui(0), 9);
        assert_eq!(a.ui.whitespace, !ws);
        assert_eq!(a.checked(ActionId::ViewWhitespace), Some(!ws));
    }

    #[test]
    fn find_opens_the_bar_with_focus_and_goto_needs_a_tab() {
        let mut a = app();
        let _ = a.window_action(Some(1), ActionId::SearchReplace, None, Intent::ui(1), 3);
        assert!(a.ui.find.as_ref().is_some_and(|b| b.replace));
        assert!(a.focus_find);
        let _ = a.window_action(Some(1), ActionId::SearchGotoLine, None, Intent::ui(1), 4);
        assert!(a.dialog.is_none(), "no such tab, no dialog");
        let _ = a.window_action(None, ActionId::FileOpen, None, Intent::ui(0), 5);
        assert!(matches!(&a.dialog, Some(Dialog::Open { path, .. }) if path.ends_with('/')));
        a.answer(Answer::Accept);
        assert!(
            a.dialog.is_some(),
            "a directory is not a file: the dialog stays"
        );
    }

    #[test]
    fn line_and_column_input() {
        assert_eq!(parse_line_col("12"), Some((12, None)));
        assert_eq!(parse_line_col(" 3:7 "), Some((3, Some(7))));
        assert_eq!(parse_line_col("0"), None);
        assert_eq!(parse_line_col("4:x"), None);
        assert_eq!(parse_line_col("abc"), None);
    }

    #[test]
    fn reload_settings_reads_the_file_again() {
        let dir = std::env::temp_dir().join(format!("ced-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ced.conf.mix");
        std::fs::write(&path, "{show_whitespace: true, font_px: 17}").unwrap();
        let mut a = app();
        a.config_path = Some(path);
        let _ = a.window_action(None, ActionId::ViewReloadSettings, None, Intent::ui(0), 1);
        assert!(a.ui.whitespace);
        assert_eq!(a.ui.font_px, 17);
        let _ = std::fs::remove_dir_all(dir);
    }
}
