// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The window's state around the controller: what the window shows beyond
//! the documents (view options, the find bar, panels, dialogs, notices), and
//! the routing of everything the controller asks of a window. No drawing
//! happens here, so it is tested without a display.
//!
//! - Every action goes through [`App::action`] (menus, shortcuts, the tab
//!   strip) or the Bus (`ced.execute`), and reaches the controller the same
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
//!   errors become desktop toasts, with the status bar as a fallback.
//! - Effects that need the Bus, the clipboard, a worker thread or the
//!   process ([`App::effects`]) are left for the shell.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use documents::actions::ActionId;
use documents::commands::{Refusal, code};
use documents::config::{self, Config, FONT_PX_MAX, FONT_PX_MIN};
use documents::controller::{Controller, Effect, Prompt, RecoveredRow, Tab};
use documents::editor::{EditorMsg, LayoutReport};
use editor::{View, Wrap};
use editor_model::mirror::Phase;
use editor_model::types::{Intent, Invoker, Level, Notice, TabId};
use serde::Serialize;
use serde_json::{Value, json};
use settings::content::{Content, default_font_size};
use toolkit::action::Origin;

/// The Mono size when nothing sets it, in points.
pub const DEFAULT_FONT_PX: u16 = 13;
/// How long an information notice stays in the status bar, in seconds.
pub const STATUS_SECONDS: f64 = 6.0;
const NOTICE_GAP_SECONDS: f64 = 5.0;

/// How a Markdown tab is shown, for this session only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewMode {
    #[default]
    Source,
    Split,
    Preview,
}

impl PreviewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Split => "split",
            Self::Preview => "preview",
        }
    }

    fn for_action(action: ActionId) -> Option<Self> {
        match action {
            ActionId::ViewMarkdownSource => Some(Self::Source),
            ActionId::ViewMarkdownSplit => Some(Self::Split),
            ActionId::ViewMarkdownPreview => Some(Self::Preview),
            _ => None,
        }
    }
}

#[derive(Default)]
struct TabPreview {
    language: String,
    source: String,
    cache: toolkit::markdown::Preview<TabId>,
    #[cfg(test)]
    reads: usize,
}

struct TabHint {
    id: TabId,
    language: String,
    mode: PreviewMode,
}

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
    pub macros_open: bool,
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
    /// Informational: its Bus action answered when it opened (F6, 2026-10-30).
    Keys,
    /// Informational: its Bus action answered when it opened (F6, 2026-10-30).
    About,
}

impl Dialog {
    /// The window-only action this dialog answers, if any.
    fn token(&self) -> Option<u64> {
        match self {
            Dialog::Open { token, .. }
            | Dialog::SaveAs { token, .. }
            | Dialog::Goto { token, .. } => Some(*token),
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
    pub mail_chooser: Option<(TabId, toolkit::file_chooser::FileChooser)>,
    /// Prompts waiting for the open dialog to close.
    queued: VecDeque<Dialog>,
    recent_notices: HashMap<(Option<TabId>, String), f64>,
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
    /// Commands run outside the registry (by the Bus, the editor, the find
    /// bar), waiting for the shell to record them, in order.
    pub noted: Vec<(String, Origin)>,
    /// Whether the last window action answered with a refusal.
    refused: bool,
    /// The editor's saved style ([`settings::content`]): its padding, size
    /// and light mode. The shell sets it from settingsd.
    pub editor_style: Content,
    /// View state only: never written to the document session.
    preview_modes: HashMap<TabId, PreviewMode>,
    /// Each tab keeps its own cache, cleared when its language changes.
    previews: HashMap<TabId, TabPreview>,
    /// Last echoed inputs. Idle frames compare borrowed values only.
    tab_hints: Vec<TabHint>,
    hint_window: Option<(Option<TabId>, bool)>,
    #[cfg(test)]
    hint_syncs: usize,
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
            mail_chooser: None,
            queued: VecDeque::new(),
            recent_notices: HashMap::new(),
            status: None,
            effects: Vec::new(),
            now: 0.0,
            theme: toolkit::theme_menu::Choice::default(),
            editor_id: None,
            editor_held: false,
            focus_find: false,
            noted: Vec::new(),
            refused: false,
            editor_style: Content::default(),
            preview_modes: HashMap::new(),
            previews: HashMap::new(),
            tab_hints: Vec::new(),
            hint_window: None,
            #[cfg(test)]
            hint_syncs: 0,
        }
    }

    /// The editor's base size in pixels. The profile's size counts once it
    /// names one; until then the config file's, else the default.
    pub fn editor_base_px(&self) -> u16 {
        let size = self.editor_style.font.size;
        if size == default_font_size() {
            return self.config.font_px.unwrap_or(DEFAULT_FONT_PX);
        }
        (size.round() as u16).clamp(FONT_PX_MIN, FONT_PX_MAX)
    }

    /// Take the editor's style from settingsd. The size starts again from
    /// it, so a zoom made before does not outlive the profile's size.
    pub fn set_editor_style(&mut self, style: Content) {
        self.editor_style = style;
        self.ui.font_px = self.editor_base_px();
    }

    fn ui_from(config: &Config, problems: bool, output: bool, find: Option<FindBar>) -> UiState {
        UiState {
            macros_open: false,
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

    fn markdown(&self, tab: TabId) -> bool {
        self.tab(tab).is_some_and(|t| t.language() == "markdown")
    }

    /// A language change leaves a saved choice dormant until it is Markdown again.
    pub fn preview_mode(&self, tab: TabId) -> PreviewMode {
        if self.markdown(tab) {
            self.preview_modes.get(&tab).copied().unwrap_or_default()
        } else {
            PreviewMode::Source
        }
    }

    /// The parsed view text, invalidated by edits and language changes.
    pub fn preview_document(
        &mut self,
        tab: TabId,
    ) -> Option<std::sync::Arc<toolkit::markdown::Document>> {
        let t = self.ctl.tabs().iter().find(|t| t.id == tab)?;
        let m = t.mirror.as_ref()?;
        let preview = self.previews.entry(tab).or_default();
        if preview.language != t.language() {
            preview.cache.clear();
            preview.language.clear();
            preview.language.push_str(t.language());
        }
        if let Some(document) = preview.cache.cached_for(&tab, m.view_gen()) {
            return Some(std::sync::Arc::clone(document));
        }
        preview.source.clear();
        m.text().read(0..m.text().len(), &mut preview.source);
        #[cfg(test)]
        {
            preview.reads += 1;
        }
        Some(
            preview
                .cache
                .document(tab, m.view_gen(), || &preview.source),
        )
    }

    /// Refresh the controller's echoes after document or window state changes.
    pub fn sync_view_hints(&mut self) -> bool {
        let window = (self.active(), self.dialog.is_some());
        let mode_for = |tab: &Tab| {
            if tab.language() == "markdown" {
                self.preview_modes.get(&tab.id).copied().unwrap_or_default()
            } else {
                PreviewMode::Source
            }
        };
        let tabs = self.ctl.tabs();
        if self.hint_window == Some(window)
            && tabs.len() == self.tab_hints.len()
            && tabs.iter().zip(&self.tab_hints).all(|(tab, hint)| {
                tab.id == hint.id && tab.language() == hint.language && mode_for(tab) == hint.mode
            })
        {
            return false;
        }
        #[cfg(test)]
        {
            self.hint_syncs += 1;
        }
        let ids: std::collections::HashSet<_> = tabs.iter().map(|t| t.id).collect();
        self.preview_modes.retain(|id, _| ids.contains(id));
        self.previews.retain(|id, _| ids.contains(id));
        self.tab_hints.clear();
        for tab in self.ctl.tabs() {
            let language = tab.language();
            let mode = if language == "markdown" {
                self.preview_modes.get(&tab.id).copied().unwrap_or_default()
            } else {
                PreviewMode::Source
            };
            if let Some(preview) = self.previews.get_mut(&tab.id)
                && preview.language != language
            {
                preview.cache.clear();
                preview.language.clear();
                preview.language.push_str(language);
            }
            self.tab_hints.push(TabHint {
                id: tab.id,
                language: language.to_owned(),
                mode,
            });
        }
        for hint in &self.tab_hints {
            self.ctl
                .set_view_hint(hint.id, "preview", hint.mode.as_str());
        }
        for action in [
            ActionId::ViewMarkdownSource,
            ActionId::ViewMarkdownSplit,
            ActionId::ViewMarkdownPreview,
        ] {
            self.ctl.set_action_enabled(action, self.enabled(action));
        }
        self.hint_window = Some(window);
        true
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
            FileSend => self.active().is_some_and(|tab| self.ctl.can_send(tab)),
            MailHeaders => self
                .active_tab()
                .and_then(|tab| tab.mail.as_ref())
                .is_some_and(|mail| !mail.saving && !mail.sending),
            ViewMarkdownSource | ViewMarkdownSplit | ViewMarkdownPreview => {
                self.active().is_some_and(|tab| self.markdown(tab))
            }
            FileNew | FileOpen | FileExit | FileSaveAll | HelpKeys | HelpMacros | HelpAbout
            | ViewZoomIn | ViewZoomOut | ViewZoomReset | ViewWhitespace | ViewLineNumbers
            | ViewRemoteCarets | ViewProblems | ViewOutput | ViewReloadSettings => true,
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
        if let Some(mode) = PreviewMode::for_action(action) {
            return Some(
                self.active()
                    .is_some_and(|tab| self.preview_mode(tab) == mode),
            );
        }
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
                    // The Bus asked and the window ran it: one run, unless
                    // the window refused it (no document, say).
                    let bus = matches!(intent.by, Invoker::Bus { .. });
                    self.refused = false;
                    let more = self.window_action(tab, action, args, intent, token);
                    if bus && !self.refused {
                        self.noted.push((action.id(), Origin::Bus));
                    }
                    work.extend(more);
                }
                Effect::Notice { tab, notice } => self.notice(tab, notice),
                Effect::Executed { id } => self.noted.push((id, Origin::Bus)),
                other => self.effects.push(other),
            }
        }
        self.sync_view_hints();
    }

    fn notice(&mut self, tab: Option<TabId>, notice: Notice) {
        // Conflicts and detached copies are drawn from the mirror's state.
        let Notice::Message { level, text } = notice else {
            return;
        };
        let text = if text.trim().is_empty() {
            crate::label("request-failed")
        } else {
            text
        };
        if level == Level::Info {
            self.status = Some((text, self.now + STATUS_SECONDS));
            return;
        }
        let text = match tab.and_then(|id| self.tab(id)) {
            Some(tab) => format!("{}: {text}", crate::view::tab_name(tab)),
            None => text,
        };
        self.recent_notices
            .retain(|_, at| self.now - *at < NOTICE_GAP_SECONDS);
        let key = (tab, text.clone());
        if self.recent_notices.contains_key(&key) {
            return;
        }
        self.recent_notices.insert(key, self.now);
        self.effects.push(Effect::Notify { text, level });
    }

    /// The status line, while it lasts.
    pub fn status(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|(_, until)| *until > self.now)
            .map(|(text, _)| text.as_str())
    }

    pub fn notification_failed(&mut self, text: String) {
        self.status = Some((text, self.now + STATUS_SECONDS));
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
        self.sync_view_hints();
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
        self.refused = result.is_err();
        self.ctl.ui_done(token, result)
    }

    fn refused(error_code: &str, reason: &str) -> Refusal {
        let message = match reason {
            "cancelled" => "cancelled in the window",
            "no_document" => "no document is open",
            "disabled" => "the command is disabled",
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
        if let Some(mode) = PreviewMode::for_action(action) {
            let Some(tab) = tab.filter(|tab| self.markdown(*tab)) else {
                return self.done(token, Err(Self::refused(code::CONFLICT, "disabled")));
            };
            if self.dialog.is_some() {
                return self.done(token, Err(Self::refused(code::CONFLICT, "disabled")));
            }
            self.preview_modes.insert(tab, mode);
            return self.done(token, ok());
        }
        match action {
            ViewZoomIn | ViewZoomOut | ViewZoomReset => {
                self.ui.font_px = match action {
                    ViewZoomIn => (self.ui.font_px + 1).min(FONT_PX_MAX),
                    ViewZoomOut => self.ui.font_px.saturating_sub(1).max(FONT_PX_MIN),
                    _ => self.editor_base_px(),
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
                    // The controller refuses a search with no document: that
                    // is not a run to record (the reply is unchanged).
                    let refused = tab.filter(|t| self.tab_exists(*t)).is_none();
                    let args = self.find_args();
                    let mut fx = self.ctl.on_action_args(tab, action, Some(args), intent);
                    fx.extend(self.done(token, ok()));
                    self.refused = refused;
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
            HelpMacros => {
                self.ui.macros_open = !self.ui.macros_open;
                self.done(token, ok())
            }
            // Informational dialogs answer when they open: a caller has no reason
            // to wait for a person to close them (F6 ruling, 2026-10-30). Input
            // dialogs keep their token and answer with the result.
            HelpKeys | HelpAbout => {
                let queued = self.dialog.is_some();
                self.show(if matches!(action, HelpKeys) {
                    Dialog::Keys
                } else {
                    Dialog::About
                });
                self.done(token, Ok(json!({"opened": true, "queued": queued})))
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
    pub fn find(&mut self, action: ActionId, origin: Origin) {
        if self.ui.find.as_ref().is_some_and(|b| !b.pattern.is_empty()) {
            let args = self.find_args();
            // With no document the controller refuses it: no run.
            if self.active().is_some() {
                self.noted.push((action.id(), origin));
            }
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
            (Dialog::Keys | Dialog::About, _) => Vec::new(),
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
            editor::Event::Undo => return self.editor_key(ActionId::EditUndo),
            editor::Event::Redo => return self.editor_key(ActionId::EditRedo),
            editor::Event::Clipboard(kind) => {
                let action = match kind {
                    editor::Clipboard::Copy => ActionId::EditCopy,
                    editor::Clipboard::Cut => ActionId::EditCut,
                    editor::Clipboard::Paste => ActionId::EditPaste,
                };
                // The widget did the work; this records it.
                self.noted.push((action.id(), Origin::Shortcut));
                return;
            }
        };
        let fx = self.ctl.on_editor(tab, msg);
        self.absorb(fx);
    }

    /// A key the editor widget passed on (Undo, Redo): run and record it.
    fn editor_key(&mut self, action: ActionId) {
        if self.enabled(action) {
            self.noted.push((action.id(), Origin::Shortcut));
        }
        self.action(action);
    }

    /// The editor's view options for `tab`: prose wraps, code scrolls.
    pub fn view_for(&self, tab: &Tab) -> View {
        let language = tab.language();
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

    /// Run the real controller and mirrors against the in-process edit service.
    fn settle(a: &mut App, service: &mut editor_model::fake::FakeEditd) -> Vec<Effect> {
        use editor_model::types::Incoming;
        let mut replies = Vec::new();
        while !a.effects.is_empty() {
            for effect in std::mem::take(&mut a.effects) {
                if let Effect::Send { req, out } = effect {
                    let args = serde_json::from_str(&out.body).unwrap();
                    let mut answer = service.handle("local:ced", &out.command, &args);
                    // FakeEditd models buffers, not their window holders.
                    if out.command == "edit.close" {
                        answer.rc = 0;
                        answer.body = json!({});
                    }
                    if out.command == "edit.open" && answer.rc == 0 {
                        let path = answer.body["path"].as_str().map(std::path::Path::new);
                        answer.body["language"] = json!(edit::lang::detect(path, ""));
                    }
                    let fx = a.ctl.on_incoming(Incoming::Reply {
                        req,
                        rc: answer.rc,
                        body: answer.body.to_string(),
                    });
                    a.absorb(fx);
                    for event in service.take_events() {
                        let fx = a.ctl.on_incoming(Incoming::Topic {
                            topic: "edit.changed".into(),
                            body: serde_json::to_string(&event).unwrap(),
                        });
                        a.absorb(fx);
                    }
                } else if matches!(effect, Effect::Respond { .. }) {
                    replies.push(effect);
                }
            }
        }
        replies
    }

    fn documents() -> (App, editor_model::fake::FakeEditd, TabId, TabId) {
        let mut a = app();
        let mut service = editor_model::fake::FakeEditd::new("0000e1e1");
        service.create(Some("/work/one.md"), "# One\n");
        service.create(Some("/work/two.md"), "# Two\n");
        let fx = a.ctl.open_paths(
            &["/work/one.md".into(), "/work/two.md".into()],
            Intent::ui(0),
        );
        a.absorb(fx);
        settle(&mut a, &mut service);
        let one = a.ctl.tabs()[0].id;
        let two = a.ctl.tabs()[1].id;
        let fx = a.ctl.select_tab(one);
        a.absorb(fx);
        (a, service, one, two)
    }

    fn bus(
        a: &mut App,
        service: &mut editor_model::fake::FakeEditd,
        command_name: &str,
        body: Value,
    ) -> (u8, Value) {
        a.sync_view_hints();
        let fx = a.ctl.on_bus_command(documents::controller::BusCommand {
            id: 7,
            command: command_name.into(),
            body: body.to_string(),
            caller_key: "local:test".into(),
        });
        a.absorb(fx);
        settle(a, service)
            .into_iter()
            .find_map(|effect| match effect {
                Effect::Respond { rc, body, .. } => {
                    Some((rc, serde_json::from_str(&body).unwrap()))
                }
                _ => None,
            })
            .expect("a Bus reply")
    }

    #[test]
    fn markdown_modes_are_per_tab_and_closing_drops_the_mode_and_cache() {
        let (mut a, mut service, one, two) = documents();
        assert_eq!(a.preview_mode(one), PreviewMode::Source);
        a.action(ActionId::ViewMarkdownSplit);
        assert_eq!(a.preview_mode(one), PreviewMode::Split);
        assert_eq!(a.preview_mode(two), PreviewMode::Source);
        let first = a.preview_document(one).unwrap();
        let fx = a.ctl.select_tab(two);
        a.absorb(fx);
        a.action(ActionId::ViewMarkdownPreview);
        a.preview_document(two).unwrap();
        assert_eq!(a.preview_mode(two), PreviewMode::Preview);
        assert!(std::sync::Arc::ptr_eq(
            &first,
            &a.preview_document(one).unwrap()
        ));
        let (rc, _) = bus(
            &mut a,
            &mut service,
            "ced.execute",
            json!({"id":"file.close","params":{"tab":one,"args":{"force":true}}}),
        );
        assert_eq!(rc, 0);
        assert!(!a.preview_modes.contains_key(&one));
        assert!(!a.previews.contains_key(&one));
        assert_eq!(a.preview_mode(two), PreviewMode::Preview);
    }

    #[test]
    fn markdown_cache_tracks_edits_and_language_but_not_repaints() {
        let (mut a, mut service, one, _) = documents();
        a.action(ActionId::ViewMarkdownSplit);
        let first = a.preview_document(one).unwrap();
        let reads = a.previews[&one].reads;
        assert!(std::sync::Arc::ptr_eq(
            &first,
            &a.preview_document(one).unwrap()
        ));
        assert_eq!(a.previews[&one].reads, reads, "a repaint reads no text");
        let (rc, _) = bus(&mut a, &mut service, "ced.type", json!({"text":"new "}));
        assert_eq!(rc, 0);
        let edited = a.preview_document(one).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&first, &edited));
        let gen_before = a.tab(one).unwrap().mirror.as_ref().unwrap().view_gen();
        let (rc, _) = bus(
            &mut a,
            &mut service,
            "ced.execute",
            json!({"id":"file.save_as","params":{"args":{"path":"/work/one.txt"}}}),
        );
        assert_eq!(rc, 0);
        assert_eq!(
            a.tab(one).unwrap().mirror.as_ref().unwrap().view_gen(),
            gen_before
        );
        assert_eq!(a.preview_mode(one), PreviewMode::Source);
        assert_eq!(a.preview_modes[&one], PreviewMode::Split);
        assert!(a.previews[&one].cache.cached().is_none());
        let language = a.preview_document(one).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&edited, &language));
        for action in [
            ActionId::ViewMarkdownSource,
            ActionId::ViewMarkdownSplit,
            ActionId::ViewMarkdownPreview,
        ] {
            assert!(!a.enabled(action));
        }
        assert_eq!(
            bus(&mut a, &mut service, "ced.state", json!({})).1["preview"],
            "source"
        );
        let (rc, _) = bus(
            &mut a,
            &mut service,
            "ced.execute",
            json!({"id":"file.save_as","params":{"args":{"path":"/work/one.md"}}}),
        );
        assert_eq!(rc, 0);
        assert_eq!(a.preview_mode(one), PreviewMode::Split);
        assert!(!std::sync::Arc::ptr_eq(
            &edited,
            &a.preview_document(one).unwrap()
        ));
    }

    #[test]
    fn bus_markdown_action_and_view_hints_match_the_window() {
        let (mut a, mut service, one, _) = documents();
        let (rc, _) = bus(
            &mut a,
            &mut service,
            "ced.execute",
            json!({"id":"view.markdown.split"}),
        );
        assert_eq!(rc, 0);
        assert_eq!(a.checked(ActionId::ViewMarkdownSplit), Some(true));
        assert_eq!(
            bus(&mut a, &mut service, "ced.tabs", json!({})).1["tabs"][0]["preview"],
            "split"
        );
        assert_eq!(
            bus(&mut a, &mut service, "ced.state", json!({})).1["preview"],
            "split"
        );
        assert!(
            a.noted
                .iter()
                .any(|(id, origin)| id == "view.markdown.split" && *origin == Origin::Bus)
        );
        bus(
            &mut a,
            &mut service,
            "ced.execute",
            json!({"id":"file.save_as","params":{"args":{"path":"/work/one.txt"}}}),
        );
        let (rc, refusal) = bus(
            &mut a,
            &mut service,
            "ced.execute",
            json!({"id":"view.markdown.split","params":{"tab":one}}),
        );
        assert_eq!(rc, 10);
        assert_eq!(refusal["error_code"], "CONFLICT");
        assert_eq!(refusal["reason"], "disabled");
        let actions = bus(&mut a, &mut service, "ced.actions", json!({})).1;
        let rows: Vec<_> = actions["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["id"].as_str().unwrap().starts_with("view.markdown."))
            .collect();
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|row| row["enabled"] == false && row["keys"] == json!([]))
        );
    }

    #[test]
    fn no_tab_disables_markdown_commands_without_panicking() {
        let a = app();
        for action in [
            ActionId::ViewMarkdownSource,
            ActionId::ViewMarkdownSplit,
            ActionId::ViewMarkdownPreview,
        ] {
            assert!(!a.enabled(action));
            assert_eq!(a.checked(action), Some(false));
            assert!(documents::keymap::chords_for(action).is_empty());
        }
    }

    #[test]
    fn idle_frames_do_not_write_view_hints_or_enablement() {
        let (mut a, _, one, _) = documents();
        a.action(ActionId::ViewMarkdownSplit);
        let syncs = a.hint_syncs;
        for _ in 0..2 {
            a.prune();
            a.absorb(Vec::new());
            assert!(!a.sync_view_hints());
            assert_eq!(a.hint_syncs, syncs, "idle frames do not reach the writes");
        }
        a.dialog = Some(Dialog::About);
        assert!(a.sync_view_hints());
        assert!(!a.sync_view_hints());
        a.dialog = None;
        assert!(a.sync_view_hints());
        assert_eq!(a.preview_mode(one), PreviewMode::Split);
    }

    #[test]
    fn informational_dialogs_answer_when_they_open_and_closing_answers_nothing() {
        for (id, is_keys) in [("help.about", false), ("help.keys", true)] {
            let (mut a, mut service, _, _) = documents();
            let (rc, body) = bus(&mut a, &mut service, "ced.execute", json!({"id": id}));
            assert_eq!(rc, 0, "{id}");
            assert_eq!(body["opened"], true, "{id}");
            assert_eq!(body["queued"], false, "{id}");
            assert!(match a.dialog {
                Some(Dialog::Keys) => is_keys,
                Some(Dialog::About) => !is_keys,
                _ => false,
            });
            // Closing it later must not answer a second time.
            a.answer(Answer::Accept);
            assert!(a.dialog.is_none());
            assert!(
                settle(&mut a, &mut service)
                    .into_iter()
                    .all(|effect| !matches!(effect, Effect::Respond { .. })),
                "{id}: closing answered again"
            );
        }
    }

    #[test]
    fn an_input_dialog_that_outlives_its_caller_answers_once_with_timeout() {
        use editor_model::types::Incoming;
        let (mut a, mut service, one, _) = documents();
        let fx = a.ctl.on_bus_command(documents::controller::BusCommand {
            id: 9,
            command: "ced.execute".into(),
            body: json!({"id": "search.goto_line", "params": {"tab": one}}).to_string(),
            caller_key: "local:test".into(),
        });
        let deadline = fx.iter().find_map(|effect| match effect {
            Effect::Timer { id, .. } => Some(*id),
            _ => None,
        });
        a.absorb(fx);
        assert!(
            settle(&mut a, &mut service).is_empty(),
            "waits for the window"
        );
        assert!(matches!(a.dialog, Some(Dialog::Goto { .. })));
        // The caller's deadline passes before anyone answers the dialog.
        let fx = a.ctl.on_incoming(Incoming::Timer {
            id: deadline.expect("a UI deadline timer"),
        });
        let timeouts = fx
            .iter()
            .filter(|effect| matches!(effect, Effect::Respond { rc: 10, body, .. } if body.contains("TIMEOUT")))
            .count();
        assert_eq!(timeouts, 1);
        a.absorb(fx);
        settle(&mut a, &mut service);
        // The person finishes it later: the line is gone to, nobody is answered again.
        if let Some(Dialog::Goto { text, .. }) = a.dialog.as_mut() {
            *text = "1".into();
        }
        a.answer(Answer::Accept);
        assert!(a.dialog.is_none());
        assert!(
            settle(&mut a, &mut service).is_empty(),
            "a late completion answered the timed-out caller"
        );
    }

    #[test]
    fn older_bus_actions_keep_their_enablement_with_a_window_and_dialog() {
        let (mut a, mut service, _, _) = documents();
        for dialog in [None, Some(Dialog::About)] {
            a.dialog = dialog;
            let actions = bus(&mut a, &mut service, "ced.actions", json!({})).1;
            for row in actions["actions"].as_array().unwrap() {
                if !row["id"].as_str().unwrap().starts_with("view.markdown.")
                    && !matches!(row["id"].as_str().unwrap(), "file.send" | "mail.headers")
                {
                    assert_eq!(row["enabled"], true, "{}", row["id"]);
                }
            }
        }
        assert!(!a.enabled(ActionId::EditUndo));
    }

    /// A search refused because no document is open is not recorded,
    /// whether the find bar or the Bus asked for it.
    #[test]
    fn refused_searches_are_not_recorded() {
        let mut a = app();
        a.ui.find = Some(FindBar {
            pattern: "x".into(),
            ..FindBar::default()
        });
        a.find(ActionId::SearchFindNext, Origin::Shortcut);
        assert!(a.noted.is_empty(), "{:?}", a.noted);
        a.absorb(vec![Effect::UiAction {
            tab: None,
            action: ActionId::SearchFindNext,
            args: None,
            intent: Intent::bus(0, "local:test"),
            token: 7,
        }]);
        assert!(a.noted.is_empty(), "{:?}", a.noted);
        // An accepted window action is recorded.
        a.absorb(vec![Effect::UiAction {
            tab: None,
            action: ActionId::ViewZoomIn,
            args: None,
            intent: Intent::bus(0, "local:test"),
            token: 8,
        }]);
        assert_eq!(a.noted, vec![("view.zoom_in".to_owned(), Origin::Bus)]);
    }

    #[test]
    fn notices_use_toasts_with_rate_limit_and_status_fallback() {
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
        assert_eq!(
            a.effects
                .iter()
                .filter(|fx| matches!(fx, Effect::Notify { .. }))
                .count(),
            6
        );
        a.effects.clear();
        a.notice(
            Some(1),
            Notice::Message {
                level: Level::Error,
                text: "w5".into(),
            },
        );
        assert!(
            a.effects.is_empty(),
            "identical text is limited across levels"
        );
        a.now += 5.0;
        a.notice(
            Some(1),
            Notice::Message {
                level: Level::Error,
                text: "w5".into(),
            },
        );
        assert!(
            matches!(&a.effects[0], Effect::Notify { text, level: Level::Error } if text == "w5")
        );
        a.notification_failed("w5".into());
        assert_eq!(a.status(), Some("w5"));
        a.notice(
            None,
            Notice::Message {
                level: Level::Warn,
                text: "  ".into(),
            },
        );
        assert!(
            matches!(a.effects.last().unwrap(), Effect::Notify { text, .. } if !text.trim().is_empty())
        );
    }

    #[test]
    fn document_toasts_include_the_document_name() {
        let (mut a, _, tab, _) = documents();
        a.effects.clear();
        a.notice(
            Some(tab),
            Notice::Message {
                level: Level::Error,
                text: "Save failed.".into(),
            },
        );
        assert!(
            matches!(&a.effects[0], Effect::Notify { text, .. } if text == "one.md: Save failed.")
        );
    }

    #[test]
    fn identical_notices_in_different_tabs_are_independent_and_expire() {
        let mut a = app();
        for tab in [Some(1), Some(2), None] {
            for _ in 0..2 {
                a.notice(
                    tab,
                    Notice::Message {
                        level: Level::Error,
                        text: "same.txt: failed".into(),
                    },
                );
            }
        }
        assert_eq!(
            a.effects
                .iter()
                .filter(|e| matches!(e, Effect::Notify { .. }))
                .count(),
            3
        );
        assert_eq!(a.recent_notices.len(), 3);
        a.now += NOTICE_GAP_SECONDS;
        a.notice(
            None,
            Notice::Message {
                level: Level::Error,
                text: "new error".into(),
            },
        );
        assert_eq!(a.recent_notices.len(), 1);
    }

    #[test]
    fn prompts_wait_behind_an_open_dialog() {
        let mut a = app();
        a.dialog = Some(Dialog::About);
        a.prompt(Prompt::Recovered {
            buffers: vec![RecoveredRow {
                buffer: "b1".into(),
                path: None,
                name: "untitled".into(),
                bytes: Some(3),
            }],
        });
        assert!(matches!(a.dialog, Some(Dialog::About)));
        a.answer(Answer::Accept);
        assert!(matches!(a.dialog, Some(Dialog::Recovered { .. })));
    }

    #[test]
    fn dialogs_for_a_missing_document_are_never_shown() {
        let mut a = app();
        a.dialog = Some(Dialog::About);
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
        a.dialog = Some(Dialog::Keys);
        a.queued.push_back(Dialog::About);
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
