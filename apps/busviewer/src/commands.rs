// SPDX-License-Identifier: MIT OR Apache-2.0
//! BusViewer's commands. The menu bar, the shortcuts and the Bus verbs
//! `busviewer.commands` / `busviewer.execute` all come from this registry.
use egui::{Key, KeyboardShortcut, Modifiers};
use inspector::{Dialog, Engine};
use toolkit::{Command, Icon, Registry};

fn idle(e: &Engine) -> bool {
    !e.busy() && e.connected && e.ui.dialog.is_none() && !e.quitting
}

fn editable(e: &Engine) -> bool {
    !e.busy() && e.ui.dialog.is_none()
}

fn undialogued(e: &Engine) -> bool {
    e.ui.dialog.is_none()
}

const fn key(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

pub fn registry() -> Registry<Engine> {
    let mut r = Registry::new();
    r.add(Command {
        id: "file.refresh",
        submenu: None,
        group: 0,
        label: "refresh",
        menu: Some("file"),
        shortcut: key(Modifiers::COMMAND, Key::R),
        icon: Some(Icon::RefreshCw),
        enabled: idle,
        run: |e| e.refresh(None),
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
        id: "edit.format",
        submenu: None,
        group: 0,
        label: "format",
        menu: Some("edit"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::F),
        icon: None,
        enabled: editable,
        run: Engine::format_body,
    })
    .add(Command {
        id: "edit.clear",
        submenu: None,
        group: 0,
        label: "clear",
        menu: Some("edit"),
        shortcut: None,
        icon: Some(Icon::X),
        enabled: editable,
        run: Engine::clear_body,
    })
    .add(Command {
        id: "edit.copy",
        submenu: None,
        group: 1,
        label: "copy",
        menu: Some("edit"),
        shortcut: None,
        icon: Some(Icon::Copy),
        enabled: undialogued,
        run: Engine::copy_reply,
    });
    // View › Theme: any scheme and mode, in this window only.
    r.theme_menu(
        "view",
        |e: &Engine| (e.ui.theme_scheme, e.ui.theme_mode),
        Engine::set_theme,
    );
    r.add(Command {
        id: "bus.call",
        submenu: None,
        group: 0,
        label: "call",
        menu: Some("bus"),
        shortcut: key(Modifiers::COMMAND, Key::Enter),
        icon: Some(Icon::Play),
        enabled: Engine::callable,
        run: Engine::call_selected,
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
    // The title bar's right-hand controls (no menu of their own).
    .add(Command {
        id: "view.search",
        submenu: None,
        group: 0,
        label: "search-services",
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
    fn view_theme_choices_set_the_window_theme_and_tick_it() {
        let r = registry();
        let mut e = Engine::new(crate::label);
        r.execute("view.theme.forest", &mut e).unwrap();
        r.execute("view.mode.dark", &mut e).unwrap();
        assert_eq!(
            (e.ui.theme_scheme, e.ui.theme_mode),
            (Some(design::Scheme::Forest), Some(design::Mode::Dark))
        );
        assert_eq!(r.checked("view.theme.forest", &e), Some(true));
        assert_eq!(r.checked("view.theme.session", &e), Some(false));
        let described = r.describe(&e, &crate::strings());
        let forest = described
            .iter()
            .find(|d| d.id == "view.theme.forest")
            .unwrap();
        assert_eq!(
            (forest.menu.as_deref(), forest.checked),
            (Some("View"), Some(true))
        );
        r.execute("view.mode.session", &mut e).unwrap();
        assert_eq!(e.ui.theme_mode, None);
    }

    #[test]
    fn a_dialog_disables_everything_but_reading() {
        let mut e = Engine::new(crate::label);
        e.open(Dialog::About);
        let r = registry();
        for id in [
            "file.refresh",
            "file.quit",
            "edit.format",
            "bus.call",
            "help.about",
        ] {
            assert!(
                r.execute(id, &mut e).is_err(),
                "{id} must be disabled under a dialog"
            );
        }
    }
}
