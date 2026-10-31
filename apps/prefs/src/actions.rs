// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Preferences' commands. The title bar, the panel's buttons, the shortcuts
//! and the Bus commands `prefs.actions` / `prefs.execute` all come from this
//! registry; `prefs.page.open` and `prefs.set` are its typed `page.open` and
//! `set`, so an agent or a Workshop macro moves through the pages and sets
//! each control by the same path as a click.
use egui::{Key, KeyboardShortcut, Modifiers};
use preferences::{Dialog, Engine};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use toolkit::action::{Class, EngineAction, Error, ParamSpec};
use toolkit::{Action, Icon, Registry};

fn undialogued(e: &Engine) -> bool {
    e.ui.dialog.is_none() && !e.picker.open && !e.quitting
}

const fn key(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

/// `page.open`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenPage {
    /// A page id from `prefs.pages` (COSMIC's kebab-case spelling works).
    pub page: String,
    /// A row id from `prefs.page`: open its drawer and bring it into view.
    #[serde(default)]
    pub row: Option<String>,
}

/// `set`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetKey {
    /// A settings key a page row writes.
    pub key: String,
    /// The new value; null resets the key.
    pub value: Value,
}

fn navigable(e: &Engine) -> Result<(), &'static str> {
    if e.can_navigate() {
        Ok(())
    } else {
        Err("a dialog is open")
    }
}

fn open_page(e: &mut Engine, params: &Value) -> Result<Value, Error> {
    let p: OpenPage = serde_json::from_value(params.clone()).map_err(|err| Error::BadParams {
        id: "page.open",
        msg: err.to_string(),
    })?;
    e.open_page(&p.page, p.row.as_deref())
        .map_err(Error::Failed)?;
    Ok(json!({"page": e.nav.page, "row": p.row, "drawer": e.nav.drawer}))
}

fn set_key(e: &mut Engine, params: &Value) -> Result<Value, Error> {
    let p: SetKey = serde_json::from_value(params.clone()).map_err(|err| Error::BadParams {
        id: "set",
        msg: err.to_string(),
    })?;
    e.set_key(&p.key, p.value).map_err(Error::Failed)?;
    Ok(json!({"key": p.key, "value": e.store.value(&p.key), "writing": e.store.writing()}))
}

fn typed(
    id: &'static str,
    label: &'static str,
    run: fn(&mut Engine, &Value) -> Result<Value, Error>,
    params: ParamSpec,
) -> EngineAction<Engine> {
    EngineAction {
        id,
        label,
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: navigable,
        run,
        params: Some(params),
        class: Class::Intent,
    }
}

pub fn registry() -> Registry<Engine> {
    let mut r = Registry::new();
    r.add_engine(typed(
        "page.open",
        "page-open",
        open_page,
        ParamSpec {
            type_name: "prefs::OpenPage",
            schema: || schemars::schema_for!(OpenPage),
        },
    ))
    .add_engine(typed(
        "set",
        "set-setting",
        set_key,
        ParamSpec {
            type_name: "prefs::SetKey",
            schema: || schemars::schema_for!(SetKey),
        },
    ))
    .add(Action {
        id: "sidebar.toggle",
        submenu: None,
        group: 0,
        label: "sidebar-toggle",
        menu: Some("view"),
        shortcut: None,
        icon: Some(Icon::PanelLeft),
        enabled: Engine::can_navigate,
        run: Engine::toggle_sidebar,
    })
    .add(Action {
        id: "page.back",
        submenu: None,
        group: 0,
        label: "page-back",
        menu: Some("view"),
        shortcut: key(Modifiers::ALT, Key::ArrowLeft),
        icon: Some(Icon::ChevronLeft),
        enabled: Engine::can_back,
        run: Engine::back,
    });
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
    .search_action(Action {
        id: "view.search",
        submenu: None,
        group: 1,
        label: "search",
        menu: Some("view"),
        shortcut: key(Modifiers::NONE, Key::Slash),
        icon: Some(Icon::Search),
        enabled: Engine::can_navigate,
        run: Engine::open_search,
    });
    r.scope("view.search", toolkit::action::Scope::Browsing);
    // View › Theme: any scheme, style, mode and framing, in this window only.
    for (id, label, menu, enabled, run) in [
        (
            "wallpaper.apply",
            "wallpaper-apply",
            None,
            Engine::can_apply_wallpaper as fn(&Engine) -> bool,
            Engine::apply_wallpaper as fn(&mut Engine),
        ),
        (
            "wallpaper.revert",
            "wallpaper-revert",
            None,
            Engine::can_revert_wallpaper,
            Engine::revert_wallpaper,
        ),
        (
            "wallpaper.refresh",
            "wallpaper-refresh",
            None,
            can_refresh_wallpaper,
            Engine::read_wallpaper,
        ),
        (
            "wallpaper.browse",
            "wallpaper-browse",
            None,
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
        menu: None,
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
        menu: None,
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
        menu: None,
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
        menu: None,
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
        menu: None,
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
        menu: None,
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
        menu: None,
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
        menu: None,
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
        menu: None,
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
    #[test]
    fn title_bar_carries_the_standard_tail() {
        registry().assert_tail(&Engine::new(crate::label_with), &crate::strings(), true);
    }
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
