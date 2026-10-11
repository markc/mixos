// SPDX-License-Identifier: MIT OR Apache-2.0
//! dopus's commands: one per action in the action table, so the menu bar,
//! the shortcuts, the toolbar, the context menu and the toolkit's Bus drive
//! reach the same path as `dopus.action`. Ids are the action ids; labels
//! come from the catalogue; shortcuts come from the effective keymap (an
//! action's first chord, and each further chord as an alias outside the
//! menus).

use egui::KeyboardShortcut;
use toolkit::{Command, Registry};

use crate::app::{App, Dialog};
use crate::keymap::Keymap;

/// `(id, label, menu, group)` for every action, in menu order.
type Row = (
    &'static str,
    &'static str,
    Option<&'static str>,
    u8,
);

macro_rules! actions {
    ($r:ident, $k:ident, $used:ident; $( $id:literal, $label:literal, $menu:expr, $group:literal; )*) => {
        $(
            {
                let row: Row = ($id, $label, $menu, $group);
                let command = Command {
                    id: row.0,
                    label: row.1,
                    menu: row.2,
                    submenu: None,
                    group: row.3,
                    shortcut: first_free($k, $id, &mut $used),
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
        _ => return None,
    })
}

/// `action`'s first chord not already taken.
fn first_free(
    keymap: &Keymap,
    action: &str,
    used: &mut Vec<KeyboardShortcut>,
) -> Option<KeyboardShortcut> {
    let shortcut = keymap
        .chords_for(action)
        .into_iter()
        .map(|c| c.shortcut())
        .find(|s| !used.contains(s))?;
    used.push(shortcut);
    Some(shortcut)
}

/// Every further chord of every action becomes a command of its own,
/// outside the menus, running the same action. Its id is the action id
/// with `#2`, `#3`….
fn aliases(r: &mut Registry<App>, keymap: &Keymap, used: &mut Vec<KeyboardShortcut>) {
    for (action, _) in crate::verbs::ACTIONS {
        let Some(primary) = r.get(action) else {
            continue;
        };
        let (label, enabled, run) = (primary.label, primary.enabled, primary.run);
        for (n, chord) in keymap.chords_for(action).into_iter().enumerate().skip(1) {
            let shortcut = chord.shortcut();
            if used.contains(&shortcut) {
                continue;
            }
            used.push(shortcut);
            // Built once at start-up; ids live for the process.
            let id: &'static str = Box::leak(format!("{action}#{}", n + 1).into_boxed_str());
            r.add(Command {
                id,
                label,
                menu: None,
                submenu: None,
                group: 0,
                shortcut: Some(shortcut),
                icon: None,
                enabled,
                run,
            });
        }
    }
}

/// The registry, with `keymap`'s chords as shortcuts.
pub fn registry(keymap: &Keymap) -> Registry<App> {
    let mut r = Registry::new();
    let mut used = Vec::new();
    let k = keymap;
    let file = Some("file");
    let view = Some("view");
    let go = Some("go");
    actions! { r, k, used;
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
    aliases(&mut r, keymap, &mut used);
    // View › Theme: any scheme, style, mode and framing, in this window only.
    r.theme_menu("view", |a: &App| a.theme, |a: &mut App, c| a.theme = c);
    r.add(Command {
        id: "help.shortcuts",
        label: "shortcuts",
        menu: Some("help"),
        submenu: None,
        group: 0,
        shortcut: Some(KeyboardShortcut::new(egui::Modifiers::NONE, egui::Key::F1))
            .filter(|s| !used.contains(s)),
        icon: None,
        enabled: |a: &App| a.dialog.is_none(),
        run: |a: &mut App| a.open(Dialog::Shortcuts),
    })
    .add(Command {
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
        let r = registry(&Keymap::default());
        let strings = crate::strings();
        for (action, _) in crate::verbs::ACTIONS {
            assert!(r.get(action).is_some(), "{action} has no command");
        }
        let known = |key: &str| toolkit::command::label_text(&strings, key) != key;
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
    fn extra_chords_are_aliases() {
        let r = registry(&Keymap::default());
        let alias = r.get("file.rename#2").expect("Ctrl+E alias");
        assert_eq!(
            alias.shortcut,
            Some(KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::E
            ))
        );
        assert!(alias.menu.is_none());
        assert!(r.get("nav.switch-pane#2").is_some(), "Tab");
    }

    #[test]
    fn a_doubled_overlay_chord_does_not_panic() {
        let mut k = Keymap::default();
        let chord = crate::keymap::parse_chord("Ctrl+B").unwrap();
        k.bindings.push((chord, "nav.home".into()));
        let r = registry(&k);
        assert!(r.get("nav.home").is_some());
    }
}
