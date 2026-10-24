// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! dopus's commands: one per action in the action table, so the menu bar,
//! the shortcuts, the toolbar, the context menu and the toolkit's Bus drive
//! reach the same path as `dopus.action`. Ids are the action ids; labels
//! come from the catalogue; shortcuts are the default chords
//! ([`crate::keymap`]): an action's first as its accelerator, the rest as
//! further chords. The person's bindings are applied over them by
//! `Registry::rebind`.

use egui::KeyboardShortcut;
use toolkit::{Action, Registry};

use crate::app::{App, Dialog};
use crate::keymap::defaults_for;

/// `(id, label, menu, group)` for every action, in menu order.
type Row = (&'static str, &'static str, Option<&'static str>, u8);

macro_rules! actions {
    ($r:ident, $used:ident; $( $id:literal, $label:literal, $menu:expr, $group:literal; )*) => {
        $(
            {
                let row: Row = ($id, $label, $menu, $group);
                let command = Action {
                    id: row.0,
                    label: row.1,
                    menu: row.2,
                    submenu: None,
                    group: row.3,
                    shortcut: first_free($id, &mut $used),
                    icon: None,
                    enabled: |a: &App| a.enabled($id),
                    run: |a: &mut App| a.action($id),
                };
                match toggle($id) {
                    Some(checked) => {
                        $r.add_toggle(command, checked);
                    }
                    None => {
                        $r.add(command);
                    }
                }
            }
        )*
    };
}

/// The tick for a toggle command.
fn toggle(id: &str) -> Option<fn(&App) -> bool> {
    Some(match id {
        "view.toggle-places" => |a: &App| a.checked("view.toggle-places") == Some(true),
        "view.toggle-properties" => |a: &App| a.checked("view.toggle-properties") == Some(true),
        "view.toggle-hidden" => |a: &App| a.checked("view.toggle-hidden") == Some(true),
        "view.toggle-previews" => |a: &App| a.checked("view.toggle-previews") == Some(true),
        _ => return None,
    })
}

/// `action`'s first default chord not already taken.
fn first_free(action: &str, used: &mut Vec<KeyboardShortcut>) -> Option<KeyboardShortcut> {
    let shortcut = defaults_for(action)
        .into_iter()
        .find(|s| !used.contains(s))?;
    used.push(shortcut);
    Some(shortcut)
}

/// The registry, with the default chords as shortcuts.
pub fn registry() -> Registry<App> {
    let mut r = Registry::new();
    r.add(toolkit::macros::command(
        |a: &App| a.macros_open || a.dialog.is_none(),
        |a: &mut App| a.macros_open = !a.macros_open,
    ));
    let mut used = Vec::new();
    let file = Some("file");
    let view = Some("view");
    let go = Some("go");
    actions! { r, used;
        "file.open", "file-open", file, 0;
        "file.new-folder", "file-new-folder", file, 0;
        "file.rename", "file-rename", file, 1;
        "file.copy-other-pane", "file-copy", file, 1;
        "file.move-other-pane", "file-move", file, 1;
        "file.delete", "file-delete", file, 2;
        "app.quit", "quit", file, 3;
        "view.toggle-places", "view-places", view, 0;
        "view.toggle-properties", "view-properties", view, 0;
        "view.toggle-hidden", "view-hidden", view, 1;
        "view.toggle-previews", "view-previews", view, 1;
        "view.refresh", "view-refresh", view, 1;
        "view.sort-name", "view-sort-name", view, 2;
        "view.sort-size", "view-sort-size", view, 2;
        "view.sort-modified", "view-sort-modified", view, 2;
        "nav.back", "go-back", go, 0;
        "nav.forward", "go-forward", go, 0;
        "nav.parent", "go-parent", go, 0;
        "nav.home", "go-home", go, 0;
        "location.focus", "go-location", go, 1;
        "nav.switch-pane", "go-switch-pane", go, 1;
        "selection.next", "select-next", None, 0;
        "selection.previous", "select-previous", None, 0;
        "selection.first", "select-first", None, 0;
        "selection.last", "select-last", None, 0;
        "theme.mode-toggle", "theme-mode-toggle", None, 0;
        "theme.scheme-ocean", "theme-ocean", None, 0;
        "theme.scheme-crimson", "theme-crimson", None, 0;
        "theme.scheme-stone", "theme-stone", None, 0;
        "theme.scheme-forest", "theme-forest", None, 0;
        "theme.scheme-sunset", "theme-sunset", None, 0;
        "theme.scheme-mono", "theme-mono", None, 0;
    }
    for (action, _) in crate::commands::ACTIONS {
        r.add_shortcuts(action, defaults_for(action).into_iter().skip(1));
    }
    used.extend(
        crate::commands::ACTIONS
            .iter()
            .flat_map(|(action, _)| r.shortcuts_of(action)),
    );
    // View › Theme: any scheme, style, mode and framing, in this window only.
    r.theme_menu("view", |a: &App| a.theme, |a: &mut App, c| a.theme = c);
    r.add(Action {
        id: "help.shortcuts",
        label: "shortcuts",
        menu: Some("help"),
        submenu: None,
        group: 0,
        shortcut: Some(KeyboardShortcut::new(
            egui::Modifiers::COMMAND,
            egui::Key::Slash,
        ))
        .filter(|s| !used.contains(s)),
        icon: None,
        enabled: |a: &App| a.dialog.is_none(),
        run: |a: &mut App| a.open(Dialog::Shortcuts),
    })
    .add(Action {
        id: "help.about",
        label: "about",
        menu: Some("help"),
        submenu: None,
        group: 1,
        shortcut: None,
        icon: None,
        enabled: |a: &App| a.dialog.is_none(),
        run: |a: &mut App| a.open(Dialog::About),
    });
    r.search_menu("help");
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_is_a_command_and_every_label_is_in_the_catalogue() {
        let r = registry();
        let strings = crate::strings();
        for (action, _) in crate::commands::ACTIONS {
            assert!(r.get(action).is_some(), "{action} has no command");
        }
        let known = |key: &str| toolkit::action::label_text(&strings, key) != key;
        for command in r.iter() {
            assert!(
                known(command.label),
                "{} label {:?}",
                command.id,
                command.label
            );
            assert!(
                command.menu.is_none_or(|m| strings.has(m)),
                "{} menu",
                command.id
            );
        }
    }

    #[test]
    fn further_chords_run_the_action_too() {
        let r = registry();
        let chord = |t| toolkit::keymap::parse_chord(t).unwrap();
        assert_eq!(
            r.shortcuts_of("nav.parent"),
            [chord("Backspace"), chord("Alt+ArrowUp")]
        );
        assert_eq!(
            r.get("nav.parent").unwrap().shortcut,
            Some(chord("Backspace"))
        );
        assert_eq!(r.shortcuts_of("file.rename"), [chord("Ctrl+E")]);
        assert_eq!(r.shortcuts_of("view.refresh"), [chord("Ctrl+R")]);
        assert_eq!(r.shortcuts_of("nav.switch-pane"), [chord("Tab")]);
        assert!(r.get("file.rename#2").is_none(), "no alias commands");
    }

    #[test]
    fn the_profile_rebinds_over_the_defaults() {
        let mut r = registry();
        let chord = |t| toolkit::keymap::parse_chord(t).unwrap();
        let overrides = toolkit::keymap::Overrides::from([
            ("nav.home".to_owned(), vec!["Ctrl+B".to_owned()]),
            ("file.delete".to_owned(), vec![]),
        ]);
        assert!(r.rebind(&overrides).is_empty());
        assert_eq!(r.shortcuts_of("nav.home"), [chord("Ctrl+B")]);
        assert!(
            r.shortcuts_of("view.toggle-places").is_empty(),
            "Ctrl+B moved"
        );
        assert!(r.shortcuts_of("file.delete").is_empty(), "unbound");
        r.rebind(&Default::default());
        assert_eq!(r.shortcuts_of("view.toggle-places"), [chord("Ctrl+B")]);
    }
}
