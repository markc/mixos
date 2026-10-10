// SPDX-License-Identifier: MIT OR Apache-2.0
//! Prefs' commands. The menu bar, the panel's buttons, the shortcuts and
//! the Bus verbs `prefs.commands` / `prefs.execute` all come from this
//! registry.
use egui::{Key, KeyboardShortcut, Modifiers};
use preferences::{Dialog, Engine, Panel};
use toolkit::{Command, Icon, Registry};

fn undialogued(e: &Engine) -> bool {
    e.ui.dialog.is_none()
}

const fn key(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

pub fn registry() -> Registry<Engine> {
    let mut r = Registry::new();
    r.add(Command {
        id: "file.quit",
        submenu: None,
        group: 0,
        label: "quit",
        menu: Some("file"),
        shortcut: key(Modifiers::COMMAND, Key::Q),
        icon: None,
        enabled: undialogued,
        run: Engine::quit,
    })
    .add(Command {
        id: "view.panel.applications",
        submenu: None,
        group: 0,
        label: "panel-applications",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::Num1),
        icon: Some(Icon::Package),
        enabled: undialogued,
        run: |e| e.set_panel(Panel::Applications),
    })
    .add(Command {
        id: "view.refresh",
        submenu: None,
        group: 1,
        label: "refresh",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::R),
        icon: Some(Icon::RefreshCw),
        enabled: Engine::can_refresh,
        run: Engine::refresh,
    });
    // View › Theme: any scheme and mode, in this window only.
    r.theme_menu(
        "view",
        |e: &Engine| (e.ui.theme_scheme, e.ui.theme_mode),
        Engine::set_theme,
    );
    r.add(Command {
        id: "apps.check",
        submenu: None,
        group: 0,
        label: "check",
        menu: Some("applications"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::R),
        icon: Some(Icon::Search),
        enabled: Engine::can_check,
        run: Engine::check,
    })
    .add(Command {
        id: "apps.update_all",
        submenu: None,
        group: 0,
        label: "update-all",
        menu: Some("applications"),
        shortcut: None,
        icon: Some(Icon::Download),
        enabled: Engine::can_update_all,
        run: Engine::update_all,
    })
    .add(Command {
        id: "apps.install",
        submenu: None,
        group: 1,
        label: "install",
        menu: Some("applications"),
        shortcut: key(Modifiers::COMMAND, Key::I),
        icon: Some(Icon::Download),
        enabled: Engine::can_install,
        run: Engine::install_selected,
    })
    .add(Command {
        id: "apps.rollback",
        submenu: None,
        group: 1,
        label: "rollback",
        menu: Some("applications"),
        shortcut: None,
        icon: Some(Icon::Undo2),
        enabled: Engine::can_rollback,
        run: Engine::rollback_selected,
    })
    .add(Command {
        id: "apps.remove",
        submenu: None,
        group: 2,
        label: "remove",
        menu: Some("applications"),
        shortcut: None,
        icon: Some(Icon::Trash),
        enabled: Engine::can_remove,
        run: Engine::remove_selected,
    })
    .add(Command {
        id: "help.shortcuts",
        submenu: None,
        group: 0,
        label: "shortcuts",
        menu: Some("help"),
        shortcut: key(Modifiers::NONE, Key::F1),
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
    // The title bar's light/dark switch (no menu of its own).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_label_and_menu_is_in_the_catalogue() {
        let strings = crate::strings();
        // The Theme menu's labels are the toolkit's own.
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
        let mut e = Engine::new(crate::label_with);
        e.open(Dialog::About);
        let r = registry();
        for id in [
            "file.quit",
            "view.refresh",
            "apps.check",
            "apps.install",
            "help.about",
        ] {
            assert!(
                r.execute(id, &mut e).is_err(),
                "{id} must be disabled under a dialog"
            );
        }
    }
}
