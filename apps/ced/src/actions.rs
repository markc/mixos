// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! ced's commands: one per action of the action table, so the menu bar, the
//! shortcuts, the toolkit's Bus drive and `ced.execute` all reach the same
//! controller path. Ids are the action ids; labels come from the catalogue;
//! shortcuts are the default chords for the action (its first as the
//! accelerator, the rest as further chords); the person's bindings are
//! applied over them by `Registry::rebind`. Chord text follows
//! [`toolkit::keymap`].
//!
//! Cut, Copy and Paste carry no shortcut here: the editor widget takes those
//! keys itself (egui delivers them as clipboard events, not key presses).
//! Their menu rows run through the controller like every other action.

use documents::actions::ActionId;
use documents::keymap;
use egui::KeyboardShortcut;
use toolkit::{Action, Registry};

use crate::app::App;

/// The keys the editor widget handles itself: never bound here.
fn editor_owned(action: ActionId) -> bool {
    matches!(
        action,
        ActionId::EditCut | ActionId::EditCopy | ActionId::EditPaste
    )
}

/// `action`'s default chords as shortcuts, none when the editor owns the key.
fn defaults_for(action: ActionId) -> Vec<KeyboardShortcut> {
    if editor_owned(action) {
        return Vec::new();
    }
    keymap::chords_for(action)
        .into_iter()
        .filter_map(toolkit::keymap::parse_chord)
        .collect()
}

/// The keys in force for every action, for `ced.actions`: the registry's.
/// The editor's own actions (Cut, Copy, Paste) have no registry key unless
/// the profile binds one; until then their defaults are reported, which the
/// editor handles itself.
pub fn keys_in_force(r: &Registry<App>) -> std::collections::HashMap<String, Vec<String>> {
    ActionId::all()
        .into_iter()
        .map(|action| {
            let bound = r.shortcuts_of(&action.id());
            let keys = if editor_owned(action) && bound.is_empty() {
                keymap::chords_for(action)
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            } else {
                bound.iter().map(toolkit::keymap::chord_text).collect()
            };
            (action.id(), keys)
        })
        .collect()
}

/// The first chord bound to `action`, unless the editor owns that key.
fn shortcut_for(action: ActionId) -> Option<KeyboardShortcut> {
    defaults_for(action).into_iter().next()
}

macro_rules! commands {
    ($r:ident; $( $variant:ident => $id:literal, $label:literal, $menu:literal, $group:literal; )*) => {
        $(
            $r.add(Action {
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
                Action {
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
            $r.add(Action {
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
    r.add(toolkit::macros::command(
        |a: &App| a.ui.macros_open || a.enabled(ActionId::HelpMacros),
        |a: &mut App| a.ui.macros_open = !a.ui.macros_open,
    ));
    commands! { r;
        FileNew => "file.new", "file-new", "file", 0;
        FileOpen => "file.open", "file-open", "file", 0;
        FileSend => "file.send", "file-send", "file", 1;
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
    r.add(Action {
        id: "mail.headers",
        label: "mail-headers",
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: |a: &App| a.enabled(ActionId::MailHeaders),
        run: |a: &mut App| a.action(ActionId::MailHeaders),
    });
    toggles! { r;
        EditOverwrite => "edit.overwrite", "edit-overwrite", "edit", 3;
        ViewWhitespace => "view.whitespace", "view-whitespace", "view", 1;
        ViewLineNumbers => "view.line_numbers", "view-line-numbers", "view", 1;
        ViewRemoteCarets => "view.remote_carets", "view-remote-carets", "view", 1;
        ViewProblems => "view.problems", "view-problems", "view", 2;
        ViewOutput => "view.output", "view-output", "view", 2;
    }
    r.add_toggle(
        Action {
            id: "view.markdown.source",
            label: "view-markdown-source",
            menu: Some("view"),
            submenu: Some("markdown"),
            group: 4,
            shortcut: None,
            icon: Some(toolkit::Icon::FileCode),
            enabled: |a: &App| a.enabled(ActionId::ViewMarkdownSource),
            run: |a: &mut App| a.action(ActionId::ViewMarkdownSource),
        },
        |a: &App| a.checked(ActionId::ViewMarkdownSource) == Some(true),
    );
    r.add_toggle(
        Action {
            id: "view.markdown.split",
            label: "view-markdown-split",
            menu: Some("view"),
            submenu: Some("markdown"),
            group: 4,
            shortcut: None,
            icon: Some(toolkit::Icon::FileCode),
            enabled: |a: &App| a.enabled(ActionId::ViewMarkdownSplit),
            run: |a: &mut App| a.action(ActionId::ViewMarkdownSplit),
        },
        |a: &App| a.checked(ActionId::ViewMarkdownSplit) == Some(true),
    );
    r.add_toggle(
        Action {
            id: "view.markdown.preview",
            label: "view-markdown-preview",
            menu: Some("view"),
            submenu: Some("markdown"),
            group: 4,
            shortcut: None,
            icon: Some(toolkit::Icon::FileCode),
            enabled: |a: &App| a.enabled(ActionId::ViewMarkdownPreview),
            run: |a: &mut App| a.action(ActionId::ViewMarkdownPreview),
        },
        |a: &App| a.checked(ActionId::ViewMarkdownPreview) == Some(true),
    );
    for action in ActionId::all() {
        if r.get(&action.id()).is_some() {
            r.add_shortcuts(&action.id(), defaults_for(action).into_iter().skip(1));
        }
    }
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
    // The title bar's search and light/dark switch (no menu of their own).
    // Search opens the find bar; the view sets the mode from the one drawn.
    r.add(Action {
        id: "view.search",
        label: "search-find",
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: |a: &App| a.enabled(ActionId::SearchFind),
        run: |a: &mut App| a.action(ActionId::SearchFind),
    })
    .add(Action {
        id: "view.mode",
        label: "toggle-mode",
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: toolkit::action::always,
        run: |_: &mut App| {},
    });
    r.search_menu("help");
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_bar_carries_the_standard_tail() {
        let r = registry();
        let app = App::new(
            documents::controller::Controller::new(documents::config::Config::default(), 1, false),
            documents::config::Config::default(),
        );
        let items = toolkit::titlebar::icon_bar(
            &egui::Context::default(),
            &r,
            &app,
            &crate::strings(),
            &[],
            true,
        );
        let ids: Vec<_> = items.iter().map(|i| i.command).collect();
        assert_eq!(ids, ["help.macros", "view.search", "help.keys", "view.mode"]);
    }

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
    fn further_chords_run_the_action_and_toggles_keep_shortcuts() {
        let r = registry();
        let chord = |t| toolkit::keymap::parse_chord(t).unwrap();
        assert_eq!(
            r.shortcuts_of("tabs.next"),
            [chord("Ctrl+Tab"), chord("Ctrl+PageDown")]
        );
        assert!(r.get("tabs.next#2").is_none(), "no alias commands");
        let problems = r.get("view.problems").unwrap();
        assert!(problems.shortcut.is_some(), "Problems keeps Ctrl+Shift+M");
    }

    #[test]
    fn every_default_chord_is_a_key_and_the_editor_keeps_its_own() {
        // Each default is spelled as ced.actions reports it once rebound.
        for (text, action) in keymap::DEFAULT {
            let chord = toolkit::keymap::parse_chord(text)
                .unwrap_or_else(|| panic!("{text} ({action:?}) is not a key"));
            assert_eq!(&toolkit::keymap::chord_text(&chord), text, "{action:?}");
        }
        assert_eq!(
            shortcut_for(ActionId::EditCopy),
            None,
            "the editor owns Ctrl+C"
        );
        let chord = |t| toolkit::keymap::parse_chord(t).unwrap();
        assert_eq!(shortcut_for(ActionId::ViewZoomIn), Some(chord("Ctrl+=")));
        assert_eq!(
            shortcut_for(ActionId::EditMoveLineUp),
            Some(chord("Ctrl+Shift+ArrowUp"))
        );
    }

    #[test]
    fn the_profile_rebinds_over_the_defaults() {
        let mut r = registry();
        let chord = |t| toolkit::keymap::parse_chord(t).unwrap();
        let overrides =
            toolkit::keymap::Overrides::from([("file.save".to_owned(), vec!["F12".to_owned()])]);
        assert!(r.rebind(&overrides).is_empty());
        assert_eq!(r.shortcuts_of("file.save"), [chord("F12")]);
        r.rebind(&Default::default());
        assert_eq!(r.shortcuts_of("file.save"), [chord("Ctrl+S")]);
    }
}
