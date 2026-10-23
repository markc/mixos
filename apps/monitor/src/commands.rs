// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Monitor's commands. The menu bar, the shortcuts and the Bus verbs
//! `monitor.commands` / `monitor.execute` all come from this registry.
use egui::{Key, KeyboardShortcut, Modifiers};
use toolkit::{Command, Icon, Registry};
use vitals::{Dialog, Engine, Tab};

fn undialogued(e: &Engine) -> bool {
    e.ui.dialog.is_none()
}

fn on_processes(e: &Engine) -> bool {
    e.ui.dialog.is_none() && e.ui.tab == Tab::Processes
}

const fn key(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

pub fn registry() -> Registry<Engine> {
    let mut r = Registry::new();
    r.add(toolkit::macros::command(undialogued, |e: &mut Engine| {
        e.ui.macros_open = !e.ui.macros_open;
    }));
    r.add(Command {
        id: "file.refresh",
        submenu: None,
        group: 0,
        label: "refresh",
        menu: Some("file"),
        shortcut: key(Modifiers::COMMAND, Key::R),
        icon: Some(Icon::RefreshCw),
        enabled: undialogued,
        run: Engine::refresh,
    })
    .add(Command {
        id: "file.quit",
        submenu: None,
        group: 1,
        label: "quit",
        menu: Some("file"),
        shortcut: key(Modifiers::COMMAND, Key::Q),
        icon: None,
        enabled: undialogued,
        run: Engine::quit,
    })
    .add(Command {
        id: "view.processes",
        submenu: None,
        group: 0,
        label: "processes",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::Num1),
        icon: None,
        enabled: undialogued,
        run: Engine::show_processes,
    })
    .add(Command {
        id: "view.performance",
        submenu: None,
        group: 0,
        label: "performance",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::Num2),
        icon: None,
        enabled: undialogued,
        run: Engine::show_performance,
    })
    .add(Command {
        id: "view.tree",
        submenu: None,
        group: 1,
        label: "tree",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::T),
        icon: None,
        enabled: on_processes,
        run: Engine::toggle_tree,
    })
    .add(Command {
        id: "view.pause",
        submenu: None,
        group: 1,
        label: "pause",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::P),
        icon: None,
        enabled: undialogued,
        run: Engine::toggle_pause,
    });
    // View › Theme: any scheme, style, mode and framing, in this window only.
    r.theme_menu("view", choice, set_choice);
    r.add(Command {
        id: "process.end",
        submenu: None,
        group: 0,
        label: "end-task",
        menu: Some("process"),
        shortcut: key(Modifiers::NONE, Key::Delete),
        icon: Some(Icon::Square),
        enabled: Engine::can_signal,
        run: Engine::end_selected,
    })
    .add(Command {
        id: "process.kill",
        submenu: None,
        group: 0,
        label: "kill",
        menu: Some("process"),
        shortcut: key(Modifiers::SHIFT, Key::Delete),
        icon: Some(Icon::Trash),
        enabled: Engine::can_signal,
        run: Engine::kill_selected,
    })
    .add(Command {
        id: "process.copy",
        submenu: None,
        group: 1,
        label: "copy-command",
        menu: Some("process"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::C),
        icon: Some(Icon::Copy),
        enabled: Engine::has_selection,
        run: Engine::copy_command,
    })
    .add(Command {
        id: "help.shortcuts",
        submenu: None,
        group: 0,
        label: "shortcuts",
        menu: Some("help"),
        shortcut: key(Modifiers::COMMAND, Key::Slash),
        icon: None,
        enabled: undialogued,
        run: |e| e.open(Dialog::Shortcuts),
    })
    .add(Command {
        id: "help.about",
        submenu: None,
        group: 1,
        label: "about",
        menu: Some("help"),
        shortcut: None,
        icon: None,
        enabled: undialogued,
        run: |e| e.open(Dialog::About),
    })
    // The title bar's right-hand controls (no menu of their own).
    .add(Command {
        id: "view.search",
        submenu: None,
        group: 0,
        label: "search-processes",
        menu: None,
        shortcut: key(Modifiers::COMMAND, Key::F),
        icon: Some(Icon::Search),
        enabled: undialogued,
        run: Engine::focus_filter,
    })
    .add(Command {
        id: "view.mode",
        submenu: None,
        group: 0,
        label: "toggle-mode",
        menu: None,
        shortcut: None,
        icon: None,
        enabled: toolkit::command::always,
        run: Engine::toggle_mode,
    });
    // Help opens with a field that searches every command.
    r.search_menu("help");
    r
}

/// The window's theme choice, as the Theme menu reads it.
pub fn choice(e: &Engine) -> toolkit::theme_menu::Choice {
    toolkit::theme_menu::Choice {
        scheme: e.ui.theme_scheme,
        style: e.ui.theme_style,
        mode: e.ui.theme_mode,
        decorations: e.ui.theme_decorations,
        captions: e.ui.theme_captions,
    }
}

fn set_choice(e: &mut Engine, choice: toolkit::theme_menu::Choice) {
    e.ui.theme_scheme = choice.scheme;
    e.ui.theme_style = choice.style;
    e.ui.theme_mode = choice.mode;
    e.ui.theme_decorations = choice.decorations;
    e.ui.theme_captions = choice.captions;
}

#[cfg(test)]
mod tests {
    use super::*;
    use toolkit::command::Origin;

    #[test]
    fn every_label_and_menu_is_in_the_catalogue() {
        let strings = crate::strings();
        let known = |key: &str| toolkit::command::label_text(&strings, key) != key;
        for command in registry().iter() {
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
            assert!(command.submenu.is_none_or(known), "{} submenu", command.id);
        }
    }

    #[test]
    fn a_dialog_disables_everything_but_reading() {
        let mut e = Engine::new(crate::label);
        e.open(Dialog::About);
        let r = registry();
        for id in ["file.refresh", "view.tree", "process.end", "help.about"] {
            assert!(r.execute(id, &mut e, Origin::Bus).is_err(), "{id}");
        }
    }

    #[test]
    fn pages_and_tree_switch_by_command() {
        let r = registry();
        let mut e = Engine::new(crate::label);
        r.execute("view.performance", &mut e, Origin::Bus).unwrap();
        assert_eq!(e.ui.tab, Tab::Performance);
        assert!(
            r.execute("view.tree", &mut e, Origin::Bus).is_err(),
            "only on the processes page"
        );
        r.execute("view.processes", &mut e, Origin::Bus).unwrap();
        r.execute("view.tree", &mut e, Origin::Bus).unwrap();
        assert!(e.ui.tree);
        assert!(
            r.execute("process.end", &mut e, Origin::Bus).is_err(),
            "nothing selected"
        );
    }
}
