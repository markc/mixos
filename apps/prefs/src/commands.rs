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
        id: "view.panel.app-settings",
        submenu: None,
        group: 0,
        label: "panel-app-settings",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::Num3),
        icon: Some(Icon::AppWindow),
        enabled: undialogued,
        run: |e| e.set_panel(Panel::AppSettings),
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
            Section::Terminal,
            Icon::FileTerminal,
            (|e: &mut Engine| e.set_section(Section::Terminal)) as fn(&mut Engine),
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
        (
            Section::Email,
            Icon::Mail,
            (|e: &mut Engine| e.set_section(Section::Email)) as fn(&mut Engine),
        ),
    ] {
        r.add(Command {
            id: section.command(),
            label: section.label(),
            menu: Some("view"),
            submenu: Some(match section.panel() {
                Panel::Appearance => "panel-appearance",
                Panel::Applications => "panel-applications",
                Panel::AppSettings => "panel-app-settings",
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
        (
            "view.tree.app-settings",
            "tree-app-settings",
            Engine::toggle_app_settings as fn(&mut Engine),
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
        id: "terminal.reset",
        submenu: None,
        group: 2,
        label: "reset-terminal",
        menu: None,
        shortcut: None,
        icon: None,
        enabled: Engine::can_reset_terminal,
        run: Engine::reset_terminal,
    });
    // App Settings → Email's tabs.
    for (tab, run) in [
        (
            preferences::email::EmailTab::Accounts,
            (|e: &mut Engine| e.set_email_tab(preferences::email::EmailTab::Accounts))
                as fn(&mut Engine),
        ),
        (
            preferences::email::EmailTab::Mail,
            (|e: &mut Engine| e.set_email_tab(preferences::email::EmailTab::Mail))
                as fn(&mut Engine),
        ),
        (
            preferences::email::EmailTab::Reader,
            (|e: &mut Engine| e.set_email_tab(preferences::email::EmailTab::Reader))
                as fn(&mut Engine),
        ),
        (
            preferences::email::EmailTab::Compose,
            (|e: &mut Engine| e.set_email_tab(preferences::email::EmailTab::Compose))
                as fn(&mut Engine),
        ),
    ] {
        r.add(Command {
            id: tab.command(),
            label: tab.label(),
            menu: None,
            submenu: None,
            group: 0,
            shortcut: None,
            icon: None,
            enabled: undialogued,
            run,
        });
    }
    // App Settings → Email: each acts on the selected account, as its button.
    for (id, label, group, enabled, run) in [
        (
            "email.account.add",
            "email-add",
            0,
            Engine::can_add_email_account as fn(&Engine) -> bool,
            Engine::new_email_account as fn(&mut Engine),
        ),
        (
            "email.account.save",
            "email-save",
            0,
            Engine::can_save_email_account,
            Engine::save_email_account,
        ),
        (
            "email.account.revert",
            "email-revert",
            0,
            Engine::can_revert_email_account,
            Engine::revert_email_account,
        ),
        (
            "email.account.remove",
            "email-remove",
            1,
            Engine::can_remove_email_account,
            Engine::remove_email_account,
        ),
        (
            "email.account.up",
            "email-move-up",
            1,
            Engine::can_move_email_account_up,
            Engine::move_email_account_up,
        ),
        (
            "email.account.down",
            "email-move-down",
            1,
            Engine::can_move_email_account_down,
            Engine::move_email_account_down,
        ),
        (
            "email.account.default",
            "email-make-default",
            1,
            Engine::can_default_email_account,
            Engine::default_email_account,
        ),
        (
            "email.account.enable",
            "email-toggle-enabled",
            1,
            Engine::can_enable_email_account,
            Engine::toggle_email_account,
        ),
        (
            "email.password.set",
            "email-set-password",
            2,
            Engine::can_set_email_password,
            Engine::set_email_password,
        ),
        (
            "email.password.forget",
            "email-forget-password",
            2,
            Engine::can_forget_email_password,
            Engine::forget_email_password,
        ),
        (
            "email.account.connect",
            "email-connect",
            2,
            Engine::can_connect_email_account,
            Engine::connect_email_account,
        ),
        (
            "dialog.confirm",
            "dialog-confirm",
            4,
            Engine::can_confirm_dialog,
            Engine::confirm_dialog,
        ),
        (
            "dialog.cancel",
            "cancel",
            4,
            Engine::can_cancel_dialog,
            Engine::close_dialog,
        ),
        (
            "email.reader.reset",
            "email-reader-reset",
            3,
            Engine::can_reset_reader_font,
            Engine::reset_reader_font,
        ),
        (
            "email.reset",
            "email-reset",
            3,
            Engine::can_reset_email,
            Engine::reset_email,
        ),
    ] {
        r.add(Command {
            id,
            label,
            menu: None,
            submenu: None,
            group,
            shortcut: None,
            icon: None,
            enabled,
            run,
        });
    }
    r.add(Command {
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

    /// Mark's rule: every Email-section button and dialog choice is a
    /// command, and every one is named in prefs.execute's list.
    #[test]
    fn every_email_and_dialog_command_is_listed_for_execute() {
        let described = preferences::model::describe();
        let execute = described["verbs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "prefs.execute")
            .and_then(|v| v["description"].as_str())
            .unwrap()
            .to_owned();
        let ids: Vec<&str> = registry()
            .iter()
            .map(|c| c.id)
            .filter(|id| {
                id.starts_with("email.")
                    || id.starts_with("view.email.")
                    || id.starts_with("dialog.")
                    || *id == "view.panel.app-settings"
            })
            .collect();
        for id in [
            "email.account.save",
            "email.account.revert",
            "email.password.forget",
            "email.account.connect",
            "email.account.up",
            "email.account.down",
            "email.reset",
            "email.reader.reset",
            "dialog.confirm",
        ] {
            assert!(ids.contains(&id), "{id} is a command");
        }
        for id in ids {
            assert!(
                execute.contains(&format!("{id},")) || execute.contains(&format!("{id})")),
                "{id} missing from prefs.execute's list"
            );
        }
    }

    /// A confirmation's choice can be made over the Bus: dialog.confirm
    /// presses its confirm button, dialog.cancel closes it.
    #[test]
    fn a_confirmation_is_answerable_by_command() {
        let r = registry();
        let mut e = Engine::new(crate::label_with);
        assert!(
            r.execute("dialog.confirm", &mut e, toolkit::command::Origin::Bus)
                .is_err()
        );
        e.open(Dialog::ResetEmail);
        assert!(
            r.execute("dialog.cancel", &mut e, toolkit::command::Origin::Bus)
                .is_ok()
        );
        assert_eq!(e.ui.dialog, None);
        e.open(Dialog::ForgetPassword("acct-1".into()));
        assert!(
            r.execute("dialog.confirm", &mut e, toolkit::command::Origin::Bus)
                .is_ok()
        );
        assert_eq!(e.ui.dialog, None, "confirming closes it");
        e.open(Dialog::About);
        assert!(
            r.execute("dialog.confirm", &mut e, toolkit::command::Origin::Bus)
                .is_err(),
            "not a confirmation"
        );
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
