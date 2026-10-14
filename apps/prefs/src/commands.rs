// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Prefs' commands. The menu bar, the panel's buttons, the shortcuts and
//! the Bus verbs `prefs.commands` / `prefs.execute` all come from this
//! registry.
use egui::{Key, KeyboardShortcut, Modifiers};
use preferences::{Dialog, Engine, Panel, Section};
use toolkit::{Command, Icon, Registry};

fn undialogued(e: &Engine) -> bool {
    e.ui.dialog.is_none() && !e.picker.open && !e.quitting
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
        id: "view.panel.appearance",
        submenu: None,
        group: 0,
        label: "panel-appearance",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::Num2),
        icon: Some(Icon::Palette),
        enabled: undialogued,
        run: |e| e.set_panel(Panel::Appearance),
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
    for (section, icon, run) in [
        (
            Section::Themes,
            Icon::Palette,
            (|e: &mut Engine| e.set_section(Section::Themes)) as fn(&mut Engine),
        ),
        (
            Section::Fonts,
            Icon::Type,
            (|e: &mut Engine| e.set_section(Section::Fonts)) as fn(&mut Engine),
        ),
        (
            Section::Wallpapers,
            Icon::Image,
            (|e: &mut Engine| e.set_section(Section::Wallpapers)) as fn(&mut Engine),
        ),
        (
            Section::Desktop,
            Icon::Monitor,
            (|e: &mut Engine| e.set_section(Section::Desktop)) as fn(&mut Engine),
        ),
        (
            Section::Server,
            Icon::Server,
            (|e: &mut Engine| e.set_section(Section::Server)) as fn(&mut Engine),
        ),
        (
            Section::Creative,
            Icon::Palette,
            (|e: &mut Engine| e.set_section(Section::Creative)) as fn(&mut Engine),
        ),
        (
            Section::Other,
            Icon::Package,
            (|e: &mut Engine| e.set_section(Section::Other)) as fn(&mut Engine),
        ),
    ] {
        r.add(Command {
            id: section.command(),
            label: section.label(),
            menu: Some("view"),
            submenu: Some(if section.panel() == Panel::Appearance {
                "panel-appearance"
            } else {
                "panel-applications"
            }),
            group: 0,
            shortcut: None,
            icon: Some(icon),
            enabled: undialogued,
            run,
        });
    }
    for (id, label, run) in [
        (
            "view.tree.appearance",
            "tree-appearance",
            Engine::toggle_appearance as fn(&mut Engine),
        ),
        (
            "view.tree.applications",
            "tree-applications",
            Engine::toggle_applications as fn(&mut Engine),
        ),
    ] {
        r.add(Command {
            id,
            label,
            run,
            menu: None,
            submenu: None,
            group: 0,
            shortcut: None,
            icon: None,
            enabled: undialogued,
        });
    }
    // View › Theme: any scheme, style, mode and framing, in this window only.
    for (id, label, menu, enabled, run) in [
        (
            "wallpaper.apply",
            "wallpaper-apply",
            Some("wallpaper"),
            Engine::can_apply_wallpaper as fn(&Engine) -> bool,
            Engine::apply_wallpaper as fn(&mut Engine),
        ),
        (
            "wallpaper.revert",
            "wallpaper-revert",
            Some("wallpaper"),
            Engine::can_revert_wallpaper,
            Engine::revert_wallpaper,
        ),
        (
            "wallpaper.refresh",
            "wallpaper-refresh",
            Some("wallpaper"),
            can_refresh_wallpaper,
            Engine::read_wallpaper,
        ),
        (
            "wallpaper.browse",
            "wallpaper-browse",
            Some("wallpaper"),
            Engine::can_browse_wallpaper,
            Engine::browse_wallpaper,
        ),
        (
            "wallpaper.picker.go",
            "wallpaper-go",
            None,
            Engine::can_picker_go,
            Engine::picker_go,
        ),
        (
            "wallpaper.picker.parent",
            "wallpaper-parent",
            None,
            Engine::can_picker_go,
            Engine::picker_parent,
        ),
        (
            "wallpaper.picker.use",
            "wallpaper-use-image",
            None,
            Engine::can_picker_use,
            Engine::picker_use,
        ),
        (
            "wallpaper.picker.cancel",
            "cancel",
            None,
            picker_open,
            Engine::picker_cancel,
        ),
    ] {
        r.add(Command {
            id,
            label,
            menu,
            submenu: None,
            group: 0,
            shortcut: None,
            icon: None,
            enabled,
            run,
        });
    }
    r.theme_menu("view", choice, set_choice);
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
        id: "appearance.apply",
        submenu: None,
        group: 0,
        label: "apply-look",
        menu: Some("appearance"),
        shortcut: key(Modifiers::COMMAND, Key::S),
        icon: Some(Icon::Check),
        enabled: Engine::can_apply_look,
        run: Engine::apply_look,
    })
    .add(Command {
        id: "appearance.revert",
        submenu: None,
        group: 0,
        label: "revert-look",
        menu: Some("appearance"),
        shortcut: None,
        icon: Some(Icon::Undo2),
        enabled: Engine::can_revert_look,
        run: Engine::revert_look,
    })
    .add(Command {
        id: "appearance.keep",
        submenu: None,
        group: 1,
        label: "keep-look",
        menu: Some("appearance"),
        shortcut: None,
        icon: None,
        enabled: Engine::can_keep_look,
        run: Engine::keep_look,
    })
    .add(Command {
        id: "appearance.recheck",
        submenu: None,
        group: 1,
        label: "recheck-look",
        menu: Some("appearance"),
        shortcut: None,
        icon: Some(Icon::RefreshCw),
        enabled: Engine::can_recheck_look,
        run: Engine::recheck_look,
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

fn picker_open(e: &Engine) -> bool {
    e.picker.open
}
fn can_refresh_wallpaper(e: &Engine) -> bool {
    e.connected && !e.quitting && undialogued(e) && !e.wallpaper.working()
}

/// Store the Theme menu's choice as the window's.
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
                r.execute(id, &mut e, toolkit::command::Origin::Bus)
                    .is_err(),
                "{id} must be disabled under a dialog"
            );
        }
    }
}
