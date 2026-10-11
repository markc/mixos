// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's state around the core: dialogs, the location bar being
//! edited, the status line, the window's theme choice, and the routing of
//! actions and Bus commands. No drawing happens here, so it is tested
//! without a display.
//!
//! - Every action goes through [`App::action`] (menus, shortcuts, the
//!   toolbar, the context menu) or [`App::serve`] (the Bus), which share
//!   [`verbs::apply_action_in`].
//! - The core's confirm and prompt requests become dialogs; one shows at a
//!   time and the rest wait (law 3). Dismissing a dialog answers No.
//! - What needs the process or the theme ([`Effect`]) is left for the
//!   shell.

use std::collections::VecDeque;
use std::path::PathBuf;

use dopus_core::config::Sidebar;
use dopus_core::{ConfirmAnswer, CoreEvent, DopusCore, DropAction, PaneId, PromptKind};

use crate::verbs::{self, ActionReply, Applied, Refusal, Served, ServerMeta, ThemeAction};

/// A modal dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum Dialog {
    /// Yes or No (delete).
    Confirm {
        token: u64,
        message: String,
    },
    /// A name (New folder, Rename).
    Prompt {
        token: u64,
        kind: PromptKind,
        text: String,
    },
    /// Items dropped on a folder: Move, Copy or Cancel.
    Drop {
        source: PaneId,
        paths: Vec<PathBuf>,
        destination: PathBuf,
    },
    Shortcuts,
    About,
}

impl Dialog {
    /// Whether the core is waiting on this dialog's answer.
    fn core_token(&self) -> Option<u64> {
        match self {
            Dialog::Confirm { token, .. } | Dialog::Prompt { token, .. } => Some(*token),
            _ => None,
        }
    }
}

/// What the person chose in a dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The default button (Yes, OK, Move here).
    Accept,
    /// The alternate button (Copy here).
    Other,
    /// Cancel, No, Esc.
    Cancel,
}

/// Work for the shell.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Reply to Bus command `id`.
    Reply {
        id: u64,
        rc: u8,
        body: String,
    },
    /// Open a file with the desktop's handler.
    OpenFile(PathBuf),
    /// Set the window's theme by name, then reply to `id` (if a caller asked).
    ThemeSet {
        id: Option<u64>,
        scheme: Option<String>,
        mode: Option<String>,
    },
    /// A `theme.*` action, then reply to `id` as `dopus.action` does.
    ThemeAction {
        id: Option<u64>,
        action: ThemeAction,
        name: String,
    },
    Quit,
}

pub struct App {
    pub core: DopusCore,
    /// `dopus.actions.list`'s table.
    pub actions: Vec<verbs::ActionRow>,
    pub dialog: Option<Dialog>,
    queued: VecDeque<Dialog>,
    /// The location bar being edited, and its text.
    pub editing: Option<(PaneId, String)>,
    /// Give the location field the keyboard on the next frame.
    pub focus_location: bool,
    /// A transient status line; the core's info line shows when it is empty.
    pub status: Option<String>,
    /// This window's own theme choice (View › Theme).
    pub theme: toolkit::theme_menu::Choice,
    /// Work for the shell.
    pub effects: Vec<Effect>,
    pub quitting: bool,
    /// Bus drive input (`dopus.ui.*`) is playing this frame. File actions,
    /// accepting a dialog that changes files, and opening files are refused
    /// then, as `dopus.action` refuses `file.*`: the Bus never changes the
    /// filesystem through dopus.
    pub driven: bool,
}

impl App {
    pub fn new(core: DopusCore) -> Self {
        let actions = verbs::action_table(crate::keymap::default_keys);
        Self {
            core,
            actions,
            dialog: None,
            queued: VecDeque::new(),
            editing: None,
            focus_location: false,
            status: None,
            theme: toolkit::theme_menu::Choice::default(),
            effects: Vec::new(),
            quitting: false,
            driven: false,
        }
    }

    /// The status bar's line: the transient status, else the core's info.
    pub fn info(&self) -> String {
        self.status
            .clone()
            .unwrap_or_else(|| self.core.info().to_owned())
    }

    /// The window can take focus and sidebar actions from the Bus.
    pub fn window_available(&self) -> bool {
        self.dialog.is_none() && !self.quitting
    }

    /// Whether `action` can run now (menus grey it out otherwise).
    pub fn enabled(&self, action: &str) -> bool {
        if self.dialog.is_some() || self.quitting {
            return false;
        }
        let a = self.core.availability();
        let idle = !a.operation_running;
        match action {
            "file.open" => a.has_selection,
            "file.new-folder" => idle,
            "file.rename" => a.has_selection && idle && a.selection_count == 1,
            "file.copy-other-pane" | "file.move-other-pane" | "file.delete" => {
                a.has_selection && idle
            }
            "nav.back" => a.can_go_back,
            "nav.forward" => a.can_go_forward,
            "nav.parent" => a.can_go_parent,
            _ => true,
        }
    }

    /// Whether a toggle is on (menus show a tick).
    pub fn checked(&self, action: &str) -> Option<bool> {
        match action {
            "view.toggle-places" => Some(self.core.sidebar(Sidebar::Places).open),
            "view.toggle-properties" => Some(self.core.sidebar(Sidebar::Properties).open),
            "view.toggle-hidden" => Some(self.core.availability().show_hidden),
            _ => None,
        }
    }

    /// Run `action` as the person did, on the active pane.
    pub fn action(&mut self, action: &str) {
        let pane = self.core.active();
        self.action_in(action, pane);
    }

    /// Run `action` as the person did, on `pane` (a toolbar or header click
    /// activates its pane first).
    pub fn action_in(&mut self, action: &str, pane: PaneId) {
        if self.dialog.is_some() || self.quitting {
            return;
        }
        if self.driven && action.starts_with("file.") {
            self.status = Some(verbs::keyboard_only(action));
            return;
        }
        match verbs::apply_action_in(action, &mut self.core, pane) {
            Ok(Applied::Done) => {}
            Ok(Applied::ToggleSidebar(sidebar)) => self.core.toggle_sidebar(sidebar),
            Ok(Applied::LocationFocus(pane)) => self.edit_location(pane),
            Ok(Applied::Theme(theme)) => self.effects.push(Effect::ThemeAction {
                id: None,
                action: theme,
                name: action.to_owned(),
            }),
            Ok(Applied::Quit) => self.quit(),
            Err(Refusal { message, .. }) => self.status = Some(message),
        }
    }

    /// Ask to quit: the shell closes once the effects before it are done.
    pub fn quit(&mut self) {
        if !self.quitting {
            self.quitting = true;
            self.effects.push(Effect::Quit);
        }
    }

    /// Serve one Bus command, leaving its replies and effects in
    /// [`App::effects`].
    pub fn serve(&mut self, command: &verbs::Command, meta: &ServerMeta) {
        let info = buildinfo::build_info!();
        for served in verbs::serve_command(command, &mut self.core, meta, &info) {
            match served {
                Served::Reply { id, rc, body } => self.effects.push(Effect::Reply { id, rc, body }),
                Served::ToggleSidebar {
                    id,
                    sidebar,
                    action,
                } => {
                    self.core.toggle_sidebar(sidebar);
                    self.reply_action(id, action);
                }
                Served::LocationFocus { id, pane, action } => {
                    self.edit_location(pane);
                    self.reply_action(id, action);
                }
                Served::ThemeSet { id, scheme, mode } => self.effects.push(Effect::ThemeSet {
                    id: Some(id),
                    scheme,
                    mode,
                }),
                Served::ThemeAction { id, action, name } => {
                    self.effects.push(Effect::ThemeAction {
                        id: Some(id),
                        action,
                        name,
                    });
                }
                Served::Quit { .. } => self.quit(),
            }
        }
    }

    fn reply_action(&mut self, id: u64, action: String) {
        let body = serde_json::to_string(&ActionReply {
            id: action,
            ok: true,
            result: None,
        })
        .unwrap_or_default();
        self.effects.push(Effect::Reply { id, rc: 0, body });
    }

    /// Take the core's derived events: dialogs queue (law 3), `OpenFile`
    /// goes to the shell (law 4), status lines show.
    pub fn absorb(&mut self, events: Vec<CoreEvent>) {
        for event in events {
            match event {
                CoreEvent::ConfirmRequested { token, message } => {
                    self.editing = None;
                    self.show(Dialog::Confirm { token, message });
                }
                CoreEvent::PromptRequested {
                    token,
                    kind,
                    initial,
                } => {
                    self.editing = None;
                    self.show(Dialog::Prompt {
                        token,
                        kind,
                        text: initial,
                    });
                }
                CoreEvent::OpenFile(path) => self.effects.push(Effect::OpenFile(path)),
                CoreEvent::Status {
                    kind: dopus_core::StatusKind::Message,
                    text,
                    ..
                } => self.status = Some(text),
                // The core's info line (operation results among them) shows
                // again.
                CoreEvent::InfoChanged => self.status = None,
                _ => {}
            }
        }
    }

    /// Open a window dialog (help, about) unless one is up.
    pub fn open(&mut self, dialog: Dialog) {
        if self.dialog.is_none() && !self.quitting {
            self.dialog = Some(dialog);
        }
    }

    fn show(&mut self, dialog: Dialog) {
        if self.dialog.is_some() {
            self.queued.push_back(dialog);
        } else {
            self.dialog = Some(dialog);
        }
    }

    fn next_dialog(&mut self) {
        self.dialog = self.queued.pop_front();
    }

    /// Items dropped on `destination`: ask Move, Copy or Cancel.
    pub fn dropped(&mut self, source: PaneId, paths: Vec<PathBuf>, destination: PathBuf) {
        let busy = self.core.availability().operation_running;
        if self.dialog.is_some() || busy || paths.is_empty() {
            return;
        }
        self.show(Dialog::Drop {
            source,
            paths,
            destination,
        });
    }

    /// Edit the open prompt's text.
    pub fn prompt_text(&mut self) -> Option<&mut String> {
        match &mut self.dialog {
            Some(Dialog::Prompt { text, .. }) => Some(text),
            _ => None,
        }
    }

    /// What the person chose in the open dialog.
    pub fn answer(&mut self, answer: Answer) {
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        // Drive input may dismiss a dialog, never accept one that changes files.
        let changes_files = matches!(
            dialog,
            Dialog::Confirm { .. } | Dialog::Prompt { .. } | Dialog::Drop { .. }
        );
        if self.driven && changes_files && answer != Answer::Cancel {
            self.status = Some(verbs::keyboard_only("a file dialog"));
            return;
        }
        match (dialog, answer) {
            (Dialog::Confirm { token, .. }, Answer::Accept) => {
                self.core.confirm(token, ConfirmAnswer::Yes);
            }
            (Dialog::Confirm { token, .. }, _) => self.core.confirm(token, ConfirmAnswer::No),
            (Dialog::Prompt { token, text, .. }, Answer::Accept) => {
                // Law 6: an invalid name keeps the dialog open; the core
                // checks again when it resolves.
                if dopus_core::validate_filename(&text).is_err() {
                    return;
                }
                self.core.prompt_text(token, Some(text));
            }
            (Dialog::Prompt { token, .. }, _) => self.core.prompt_text(token, None),
            (
                Dialog::Drop {
                    source,
                    paths,
                    destination,
                },
                choice @ (Answer::Accept | Answer::Other),
            ) => {
                let action = if choice == Answer::Accept {
                    DropAction::Move
                } else {
                    DropAction::Copy
                };
                if let Err(error) = self.core.transfer_paths(source, paths, destination, action) {
                    self.status = Some(error);
                }
            }
            _ => {}
        }
        self.next_dialog();
    }

    /// Quitting: answer every dialog the core waits on as No.
    pub fn cancel_dialogs(&mut self) {
        let all: Vec<Dialog> = self
            .dialog
            .take()
            .into_iter()
            .chain(std::mem::take(&mut self.queued))
            .collect();
        for dialog in all {
            match dialog {
                Dialog::Confirm { token, .. } => self.core.confirm(token, ConfirmAnswer::No),
                Dialog::Prompt { token, .. } => self.core.prompt_text(token, None),
                _ => {}
            }
        }
    }

    /// Whether the core waits on a dialog's answer.
    pub fn waiting(&self) -> bool {
        self.dialog
            .iter()
            .chain(&self.queued)
            .any(|d| d.core_token().is_some())
    }

    /// Start editing `pane`'s location bar with its path.
    pub fn edit_location(&mut self, pane: PaneId) {
        if self.dialog.is_some() {
            return;
        }
        self.core.set_active_pane(pane);
        let path = self.core.pane(pane).path.display().to_string();
        self.editing = Some((pane, path));
        self.focus_location = true;
    }

    /// Go to the edited location (`~` expands; a file opens its folder).
    pub fn submit_location(&mut self) {
        if let Some((pane, text)) = self.editing.take() {
            let path = verbs::navigable(crate::dirs::expand_tilde(text.trim()));
            self.core.navigate(pane, path);
        }
    }

    pub fn cancel_location(&mut self) {
        self.editing = None;
    }

    /// Click on a row: activate its pane and select it (Ctrl toggles,
    /// Shift extends).
    pub fn click_row(&mut self, pane: PaneId, path: PathBuf, ctrl: bool, shift: bool) {
        if self.dialog.is_some() {
            return;
        }
        self.core.set_active_pane(pane);
        if ctrl || shift {
            self.core.select_modified(pane, path, ctrl, shift);
        } else {
            self.core.select_path(pane, Some(path));
        }
    }

    /// A header click: sort `pane` by `column` (law 5: a new column sorts
    /// ascending; the same column reverses in the core).
    pub fn sort(&mut self, pane: PaneId, column: dopus_core::SortColumn) {
        if self.dialog.is_some() {
            return;
        }
        self.core.set_active_pane(pane);
        self.core.set_sort_in(pane, column, true);
    }

    /// Go to `path` in the active pane (Places).
    pub fn go(&mut self, path: PathBuf) {
        if self.dialog.is_some() {
            return;
        }
        let pane = self.core.active();
        self.editing = None;
        self.core.navigate(pane, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dopus_core::DOpusConfig;

    fn app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = DOpusConfig::default();
        config.left.path = dir.path().to_owned();
        config.right.path = dir.path().to_owned();
        let (core, _rx) = DopusCore::new(config, None);
        (dir, App::new(core))
    }

    #[test]
    fn dialogs_queue_and_dismissal_answers_no() {
        let (_dir, mut a) = app();
        a.absorb(vec![
            CoreEvent::ConfirmRequested {
                token: 1,
                message: "Delete?".into(),
            },
            CoreEvent::PromptRequested {
                token: 2,
                kind: PromptKind::NewFolder,
                initial: "New folder".into(),
            },
        ]);
        assert!(matches!(a.dialog, Some(Dialog::Confirm { token: 1, .. })));
        assert!(a.waiting());
        a.answer(Answer::Cancel);
        assert!(matches!(a.dialog, Some(Dialog::Prompt { token: 2, .. })));
        a.answer(Answer::Cancel);
        assert!(a.dialog.is_none());
    }

    #[test]
    fn an_invalid_name_keeps_the_prompt() {
        let (_dir, mut a) = app();
        a.absorb(vec![CoreEvent::PromptRequested {
            token: 3,
            kind: PromptKind::Rename,
            initial: String::new(),
        }]);
        *a.prompt_text().unwrap() = "a/b".into();
        a.answer(Answer::Accept);
        assert!(a.dialog.is_some(), "a slash is not a file name");
    }

    #[test]
    fn a_dialog_blocks_actions_and_refusals_show() {
        let (_dir, mut a) = app();
        a.action("file.rename");
        assert_eq!(a.status.as_deref(), Some("Nothing is selected"));
        a.open(Dialog::About);
        assert!(!a.enabled("view.refresh"));
        a.action("view.toggle-places");
        assert_eq!(a.checked("view.toggle-places"), Some(true), "unchanged");
    }

    #[test]
    fn open_file_and_quit_reach_the_shell() {
        let (_dir, mut a) = app();
        a.absorb(vec![CoreEvent::OpenFile("/x/y.txt".into())]);
        a.action("app.quit");
        assert_eq!(
            a.effects,
            [Effect::OpenFile("/x/y.txt".into()), Effect::Quit]
        );
        assert!(!a.window_available());
    }

    #[test]
    fn the_location_bar_edits_and_navigates() {
        let (dir, mut a) = app();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        a.edit_location(PaneId::Right);
        assert_eq!(a.core.active(), PaneId::Right);
        assert!(a.focus_location);
        a.editing.as_mut().unwrap().1 = sub.display().to_string();
        a.submit_location();
        assert_eq!(a.core.pane(PaneId::Right).path, sub);
        assert!(a.editing.is_none());
    }

    #[test]
    fn bus_window_effects_reply_once() {
        let (_dir, mut a) = app();
        let meta = ServerMeta {
            service: "dopus".into(),
            headless: false,
            window_available: true,
            config_path: None,
            theme_scheme: String::new(),
            theme_mode: String::new(),
            appearance: Default::default(),
            actions: a.actions.clone(),
            extra_verbs: Vec::new(),
        };
        let open = a.checked("view.toggle-places");
        a.serve(
            &verbs::Command {
                id: 9,
                verb: "dopus.action".into(),
                body: r#"{"id":"view.toggle-places"}"#.into(),
            },
            &meta,
        );
        assert_ne!(a.checked("view.toggle-places"), open);
        assert!(matches!(
            &a.effects[..],
            [Effect::Reply { id: 9, rc: 0, .. }]
        ));
    }

    /// Drive input (`dopus.ui.*`) runs no file action and accepts no dialog
    /// that changes files; it may still dismiss one.
    #[test]
    fn drive_input_never_changes_files() {
        let (_dir, mut a) = app();
        a.driven = true;
        a.action("file.new-folder");
        assert!(a.dialog.is_none());
        assert!(a.status.as_deref().is_some_and(|s| s.contains("keyboard-only")));
        a.action("view.toggle-hidden");
        assert!(a.core.pane(a.core.active()).show_hidden, "other actions run");
        for dialog in [
            Dialog::Confirm {
                token: 7,
                message: "Delete?".into(),
            },
            Dialog::Prompt {
                token: 8,
                kind: dopus_core::PromptKind::NewFolder,
                text: "x".into(),
            },
        ] {
            a.open(dialog);
            a.answer(Answer::Accept);
            assert!(a.dialog.is_some(), "not accepted under drive input");
            a.answer(Answer::Cancel);
            assert!(a.dialog.is_none(), "dismissed");
        }
    }
}
