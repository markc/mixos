// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Preferences' commands. The menu bar, the panel's buttons, the shortcuts and
//! the Bus commands `prefs.actions` / `prefs.execute` all come from this
//! registry.
use egui::{Key, KeyboardShortcut, Modifiers};
use preferences::{Category, Dialog, Engine, SECTIONS, Section};
use toolkit::{Action, Icon, Registry};

fn undialogued(e: &Engine) -> bool {
    e.ui.dialog.is_none() && !e.picker.open && !e.quitting
}

/// Icons belong to the renderer; the engine registry stays headless.
pub fn section_icon(section: Section) -> Icon {
    match section {
        Section::Themes => Icon::Palette,
        Section::Fonts | Section::Text => Icon::Type,
        Section::Size => Icon::ZoomIn,
        Section::Motion => Icon::Play,
        Section::Wallpapers => Icon::Image,
        Section::Email => Icon::Mail,
        Section::Terminal => Icon::FileTerminal,
        Section::Software => Icon::Package,
    }
}

const fn key(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

pub fn registry() -> Registry<Engine> {
    let mut r = Registry::new();
    r.add(toolkit::macros::command(
        |s: &Engine| s.ui.macros_open || undialogued(s),
        |e: &mut Engine| {
            e.ui.macros_open = !e.ui.macros_open;
        },
    ));
    r.add(Action {
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
    .add(Action {
        id: "view.refresh",
        submenu: None,
        group: 1,
        label: "refresh",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::R),
        icon: Some(Icon::RefreshCw),
        enabled: Engine::can_refresh,
        run: Engine::refresh,
    })
    .add(Action {
        id: "view.search",
        submenu: None,
        group: 1,
        label: "search",
        menu: Some("view"),
        shortcut: key(Modifiers::NONE, Key::Slash),
        icon: Some(Icon::Search),
        enabled: Engine::can_navigate,
        run: Engine::focus_search,
    });
    r.scope("view.search", toolkit::action::Scope::Browsing);
    for (category, shortcut) in Category::ALL.into_iter().zip([
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
    ]) {
        r.add(Action {
            id: category.command(),
            label: category.label(),
            menu: Some("view"),
            submenu: None,
            group: 0,
            shortcut: key(Modifiers::COMMAND, shortcut),
            icon: None,
            enabled: category.enabled(),
            run: category.run(),
        });
    }
    for section in SECTIONS {
        r.add(Action {
            id: section.command,
            label: section.title,
            menu: Some("view"),
            submenu: Some(section.category.label()),
            group: 0,
            shortcut: None,
            icon: Some(section_icon(section.section)),
            enabled: Engine::can_navigate,
            run: section.run,
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
        r.add(Action {
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
    r.add(Action {
        id: "apps.check",
        submenu: None,
        group: 0,
        label: "check",
        menu: Some("software"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::R),
        icon: Some(Icon::Search),
        enabled: Engine::can_check,
        run: Engine::check,
    })
    .add(Action {
        id: "apps.update_all",
        submenu: None,
        group: 0,
        label: "update-all",
        menu: Some("software"),
        shortcut: None,
        icon: Some(Icon::Download),
        enabled: Engine::can_update_all,
        run: Engine::update_all,
    })
    .add(Action {
        id: "apps.install",
        submenu: None,
        group: 1,
        label: "install",
        menu: Some("software"),
        shortcut: key(Modifiers::COMMAND, Key::I),
        icon: Some(Icon::Download),
        enabled: Engine::can_install,
        run: Engine::install_selected,
    })
    .add(Action {
        id: "apps.rollback",
        submenu: None,
        group: 1,
        label: "rollback",
        menu: Some("software"),
        shortcut: None,
        icon: Some(Icon::Undo2),
        enabled: Engine::can_rollback,
        run: Engine::rollback_selected,
    })
    .add(Action {
        id: "apps.remove",
        submenu: None,
        group: 2,
        label: "remove",
        menu: Some("software"),
        shortcut: None,
        icon: Some(Icon::Trash),
        enabled: Engine::can_remove,
        run: Engine::remove_selected,
    })
    .add(Action {
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
    .add(Action {
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
    .add(Action {
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
    .add(Action {
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
    .add(Action {
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
    // Apps → Email: each acts on the selected account, as its button.
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
            "email.reset",
            "email-reset",
            3,
            Engine::can_reset_email,
            Engine::reset_email,
        ),
    ] {
        r.add(Action {
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
    r.add(Action {
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
    .add(Action {
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
    .add(Action {
        id: "view.mode",
        submenu: None,
        group: 0,
        label: "toggle-mode",
        menu: None,
        shortcut: None,
        icon: None,
        enabled: toolkit::action::always,
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
        let known = |key: &str| toolkit::action::label_text(&strings, key) != key;
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
                r.execute(id, &mut e, toolkit::action::Origin::Bus).is_err(),
                "{id} must be disabled under a dialog"
            );
        }
    }
}
