// SPDX-License-Identifier: MIT OR Apache-2.0
//! ced's commands: one per action of the action table, so the menu bar, the
//! shortcuts, the toolkit's Bus drive and `ced.action` all reach the same
//! controller path. Ids are the action ids; labels come from the catalogue;
//! shortcuts are the keymap's first chord for the action.
//!
//! Cut, Copy and Paste carry no shortcut here: the editor widget takes those
//! keys itself (egui delivers them as clipboard events, not key presses).
//! Their menu rows run through the controller like every other action.

use documents::actions::ActionId;
use documents::keymap::{self, Chord};
use egui::{Key, KeyboardShortcut, Modifiers};
use toolkit::{Command, Registry};

use crate::app::App;

/// An egui shortcut for a keymap chord, when egui has the key.
pub fn shortcut(chord: &Chord) -> Option<KeyboardShortcut> {
    let mut modifiers = Modifiers::NONE;
    if chord.ctrl {
        modifiers = modifiers.plus(Modifiers::COMMAND);
    }
    if chord.alt {
        modifiers = modifiers.plus(Modifiers::ALT);
    }
    if chord.shift {
        modifiers = modifiers.plus(Modifiers::SHIFT);
    }
    let key = match chord.key.as_str() {
        "=" => Key::Equals,
        "-" => Key::Minus,
        "/" => Key::Slash,
        "Up" => Key::ArrowUp,
        "Down" => Key::ArrowDown,
        "Left" => Key::ArrowLeft,
        "Right" => Key::ArrowRight,
        other => Key::from_name(&other.to_ascii_uppercase()).or_else(|| Key::from_name(other))?,
    };
    Some(KeyboardShortcut::new(modifiers, key))
}

/// The keys the editor widget handles itself: never bound here.
fn editor_owned(action: ActionId) -> bool {
    matches!(
        action,
        ActionId::EditCut | ActionId::EditCopy | ActionId::EditPaste
    )
}

/// Every chord of every action after its first becomes a command of its
/// own, outside the menus, running the same action: Ctrl+PageDown for Next
/// Tab, Ctrl+Shift+Z for Redo. Its id is the action id with `#2`, `#3`….
fn aliases(r: &mut Registry<App>) {
    for action in ActionId::all() {
        if editor_owned(action) {
            continue;
        }
        let chords = keymap::chords_for(action);
        for (n, chord) in chords.iter().enumerate().skip(1) {
            let Some(shortcut) = keymap::parse_chord(chord).and_then(|c| shortcut(&c)) else {
                continue;
            };
            if r.iter().any(|c| c.shortcut == Some(shortcut)) {
                continue;
            }
            // Built once at start-up; the registry keeps ids for the
            // process's lifetime.
            let id: &'static str = Box::leak(format!("{}#{}", action.id(), n + 1).into_boxed_str());
            let Some(primary) = r.get(&action.id()) else {
                continue;
            };
            let alias = Command {
                id,
                label: primary.label,
                menu: None,
                submenu: None,
                group: 0,
                shortcut: Some(shortcut),
                icon: None,
                enabled: primary.enabled,
                run: primary.run,
            };
            r.add(alias);
        }
    }
}

/// The first chord bound to `action`, unless the editor owns that key.
fn shortcut_for(action: ActionId) -> Option<KeyboardShortcut> {
    if editor_owned(action) {
        return None;
    }
    keymap::chords_for(action)
        .first()
        .and_then(|c| keymap::parse_chord(c))
        .and_then(|c| shortcut(&c))
}

macro_rules! commands {
    ($r:ident; $( $variant:ident => $id:literal, $label:literal, $menu:literal, $group:literal; )*) => {
        $(
            $r.add(Command {
                id: $id,
                label: $label,
                menu: Some($menu),
                submenu: None,
                group: $group,
                shortcut: shortcut_for(ActionId::$variant),
                icon: None,
                enabled: |a: &App| a.enabled(ActionId::$variant),
                run: |a: &mut App| a.action(ActionId::$variant),
            });
        )*
    };
}

macro_rules! toggles {
    ($r:ident; $( $variant:ident => $id:literal, $label:literal, $menu:literal, $group:literal; )*) => {
        $(
            $r.add_toggle(
                Command {
                    id: $id,
                    label: $label,
                    menu: Some($menu),
                    submenu: None,
                    group: $group,
                    shortcut: shortcut_for(ActionId::$variant),
                    icon: None,
                    enabled: |a: &App| a.enabled(ActionId::$variant),
                    run: |a: &mut App| a.action(ActionId::$variant),
                },
                |a: &App| a.checked(ActionId::$variant) == Some(true),
            );
        )*
    };
}

macro_rules! goto {
    ($r:ident; $( $n:literal => $id:literal, $label:literal; )*) => {
        $(
            $r.add(Command {
                id: $id,
                label: $label,
                menu: Some("tabs"),
                submenu: None,
                group: 1,
                shortcut: shortcut_for(ActionId::TabsGoto($n)),
                icon: None,
                enabled: |a: &App| a.enabled(ActionId::TabsGoto($n)),
                run: |a: &mut App| a.action(ActionId::TabsGoto($n)),
            });
        )*
    };
}

pub fn registry() -> Registry<App> {
    let mut r = Registry::new();
    commands! { r;
        FileNew => "file.new", "file-new", "file", 0;
        FileOpen => "file.open", "file-open", "file", 0;
        FileSave => "file.save", "file-save", "file", 1;
        FileSaveAs => "file.save_as", "file-save-as", "file", 1;
        FileSaveAll => "file.save_all", "file-save-all", "file", 1;
        FileReload => "file.reload", "file-reload", "file", 2;
        FileClose => "file.close", "file-close", "file", 3;
        FileExit => "file.exit", "file-exit", "file", 4;
        EditUndo => "edit.undo", "edit-undo", "edit", 0;
        EditRedo => "edit.redo", "edit-redo", "edit", 0;
        EditUndoOther => "edit.undo_other", "edit-undo-other", "edit", 0;
        EditUndoAny => "edit.undo_any", "edit-undo-any", "edit", 0;
        EditCut => "edit.cut", "edit-cut", "edit", 1;
        EditCopy => "edit.copy", "edit-copy", "edit", 1;
        EditPaste => "edit.paste", "edit-paste", "edit", 1;
        EditSelectAll => "edit.select_all", "edit-select-all", "edit", 1;
        EditDuplicateLine => "edit.duplicate_line", "edit-duplicate-line", "edit", 2;
        EditDeleteLine => "edit.delete_line", "edit-delete-line", "edit", 2;
        EditMoveLineUp => "edit.move_line_up", "edit-move-line-up", "edit", 2;
        EditMoveLineDown => "edit.move_line_down", "edit-move-line-down", "edit", 2;
        EditToggleComment => "edit.toggle_comment", "edit-toggle-comment", "edit", 2;
        EditIndent => "edit.indent", "edit-indent", "edit", 2;
        EditOutdent => "edit.outdent", "edit-outdent", "edit", 2;
        EditDeleteWordLeft => "edit.delete_word_left", "edit-delete-word-left", "edit", 3;
        EditDeleteWordRight => "edit.delete_word_right", "edit-delete-word-right", "edit", 3;
        SearchFind => "search.find", "search-find", "search", 0;
        SearchFindNext => "search.find_next", "search-find-next", "search", 0;
        SearchFindPrev => "search.find_prev", "search-find-prev", "search", 0;
        SearchReplace => "search.replace", "search-replace", "search", 1;
        SearchReplaceAll => "search.replace_all", "search-replace-all", "search", 1;
        SearchGotoLine => "search.goto_line", "search-goto-line", "search", 2;
        ViewZoomIn => "view.zoom_in", "view-zoom-in", "view", 0;
        ViewZoomOut => "view.zoom_out", "view-zoom-out", "view", 0;
        ViewZoomReset => "view.zoom_reset", "view-zoom-reset", "view", 0;
        ViewClearMarkers => "view.clear_markers", "view-clear-markers", "view", 3;
        ViewReloadSettings => "view.reload_settings", "view-reload-settings", "view", 3;
        TabsNext => "tabs.next", "tabs-next", "tabs", 0;
        TabsPrev => "tabs.prev", "tabs-prev", "tabs", 0;
        HelpKeys => "help.keys", "help-keys", "help", 0;
        HelpAbout => "help.about", "help-about", "help", 1;
    }
    toggles! { r;
        EditOverwrite => "edit.overwrite", "edit-overwrite", "edit", 3;
        ViewWhitespace => "view.whitespace", "view-whitespace", "view", 1;
        ViewLineNumbers => "view.line_numbers", "view-line-numbers", "view", 1;
        ViewRemoteCarets => "view.remote_carets", "view-remote-carets", "view", 1;
        ViewProblems => "view.problems", "view-problems", "view", 2;
        ViewOutput => "view.output", "view-output", "view", 2;
    }
    aliases(&mut r);
    goto! { r;
        1 => "tabs.goto.1", "tabs-goto-1";
        2 => "tabs.goto.2", "tabs-goto-2";
        3 => "tabs.goto.3", "tabs-goto-3";
        4 => "tabs.goto.4", "tabs-goto-4";
        5 => "tabs.goto.5", "tabs-goto-5";
        6 => "tabs.goto.6", "tabs-goto-6";
        7 => "tabs.goto.7", "tabs-goto-7";
        8 => "tabs.goto.8", "tabs-goto-8";
        9 => "tabs.goto.9", "tabs-goto-9";
    }
    // View › Theme: any scheme, style, mode and framing, in this window only.
    r.theme_menu("view", |a: &App| a.theme, |a: &mut App, c| a.theme = c);
    r.search_menu("help");
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_is_a_command_with_its_wire_id() {
        let r = registry();
        for action in ActionId::all() {
            assert!(
                r.get(&action.id()).is_some(),
                "{} has no command",
                action.id()
            );
        }
    }

    #[test]
    fn extra_chords_are_aliases_and_toggles_keep_shortcuts() {
        let r = registry();
        let alias = r.get("tabs.next#2").expect("Ctrl+PageDown alias");
        assert_eq!(
            alias.shortcut,
            Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::PageDown))
        );
        assert!(alias.menu.is_none(), "aliases stay out of the menus");
        let problems = r.get("view.problems").unwrap();
        assert!(problems.shortcut.is_some(), "Problems keeps Ctrl+Shift+M");
    }

    #[test]
    fn chords_become_shortcuts() {
        let chord = |s| keymap::parse_chord(s).unwrap();
        assert_eq!(
            shortcut(&chord("Ctrl+Shift+S")),
            Some(KeyboardShortcut::new(
                Modifiers::COMMAND.plus(Modifiers::SHIFT),
                Key::S
            ))
        );
        assert_eq!(
            shortcut(&chord("Ctrl+=")),
            Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::Equals))
        );
        assert_eq!(
            shortcut(&chord("Shift+F3")),
            Some(KeyboardShortcut::new(Modifiers::SHIFT, Key::F3))
        );
        assert_eq!(
            shortcut(&chord("Ctrl+PageDown")),
            Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::PageDown))
        );
        assert_eq!(
            shortcut_for(ActionId::EditCopy),
            None,
            "the editor owns Ctrl+C"
        );
    }
}
