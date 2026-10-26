// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Workshop's commands. The menu bar, the shortcuts and the Bus commands
//! `workshop.actions` / `workshop.execute` all come from this registry.
use egui::{Key, KeyboardShortcut, Modifiers};
use inspector::{Dialog, DockSide, Engine};
use toolkit::{Action, Icon, Registry};

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
    r.add(toolkit::macros::command(
        |s: &Engine| s.ui.macros_open || undialogued(s),
        |e: &mut Engine| {
            e.ui.macros_open = !e.ui.macros_open;
        },
    ));
    r.add(Action {
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
    .add(Action {
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
    .add(Action {
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
    .add(Action {
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
    .add(Action {
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
    // View › Theme: any scheme, style, mode and framing, in this window only.
    r.theme_menu("view", choice, set_choice);
    r.add(Action {
        id: "bus.call",
        submenu: None,
        group: 0,
        label: "call",
        menu: Some("bus"),
        shortcut: key(Modifiers::COMMAND, Key::Enter),
        icon: Some(Icon::Play),
        enabled: |e| Engine::callable(e) || (e.workshop.selected.is_some() && idle(e)),
        run: |e| {
            if e.workshop.selected.is_some() {
                e.macro_command("macros.run");
            } else {
                e.call_selected();
            }
        },
    });
    r.add(Action {
        id: "macro.new",
        label: "macro-new",
        menu: Some("macros-menu"),
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: Engine::can_new_macro,
        run: Engine::new_macro,
    });
    for (id, label, run) in [
        (
            "macro.put",
            "macro-save",
            (|e: &mut Engine| e.macro_command("macros.put")) as fn(&mut Engine),
        ),
        ("macro.run", "macro-run", |e: &mut Engine| {
            e.macro_command("macros.run")
        }),
        ("macro.stop", "macro-stop", |e: &mut Engine| {
            e.macro_command("macros.stop")
        }),
        ("macro.copy", "macro-copy", |e: &mut Engine| {
            e.macro_command("macros.copy")
        }),
        ("macro.move", "macro-move", |e: &mut Engine| {
            e.macro_command("macros.move")
        }),
        ("macro.remove", "macro-delete", |e: &mut Engine| {
            e.macro_command("macros.remove")
        }),
        ("macro.ced", "macro-ced", |e: &mut Engine| {
            e.macro_command("macros.ced")
        }),
        ("macro.dopus", "macro-dopus", |e: &mut Engine| {
            e.macro_command("macros.dopus")
        }),
    ] {
        r.add(Action {
            id,
            label,
            run,
            menu: Some("macros-menu"),
            submenu: None,
            group: 0,
            shortcut: if id == "macro.put" {
                key(Modifiers::COMMAND, Key::S)
            } else {
                None
            },
            icon: None,
            enabled: |e| e.workshop.selected.is_some() && !e.quitting && e.ui.dialog.is_none(),
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
    // The title bar's right-hand controls (no menu of their own).
    .add(Action {
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
    r.add(Action {
        id: "view.traffic",
        label: "traffic",
        menu: Some("view"),
        submenu: None,
        group: 1,
        shortcut: None,
        icon: None,
        enabled: undialogued,
        run: |e| e.workshop.dock_open = !e.workshop.dock_open,
    });
    for (id, label, side) in [
        ("view.traffic.auto", "traffic-auto", DockSide::Auto),
        ("view.traffic.bottom", "traffic-bottom", DockSide::Bottom),
        ("view.traffic.right", "traffic-right", DockSide::Right),
    ] {
        r.add_choice(
            toolkit::action::Place {
                id,
                label,
                menu: Some("view"),
                submenu: Some("traffic-placement"),
                group: 1,
            },
            move |e| e.set_dock_side(side),
            move |e| e.ui.dock_side == side,
        );
    }
    r.add(Action {
        id: "bus.observe",
        label: "observe-traffic",
        menu: Some("bus"),
        submenu: None,
        group: 1,
        shortcut: None,
        icon: None,
        enabled: undialogued,
        run: |e| {
            e.queue_workshop(
                "observe",
                serde_json::json!({"on":e.workshop.subscription.is_none()}),
                None,
            )
        },
    });
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

/// Store the Theme menu's choice as the window's.
pub(crate) fn set_choice(e: &mut Engine, choice: toolkit::theme_menu::Choice) {
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
    fn new_macro_is_enabled_for_groups_and_macros_only_while_idle() {
        let r = registry();
        let mut e = Engine::new(crate::label);
        let inspector::Effect::Discover { ticket } = e.take_effects().pop().unwrap() else {
            panic!("discovery")
        };
        e.discovered(ticket, inspector::Snapshot::default());
        e.take_effects();
        assert!(
            r.execute("macro.new", &mut e, toolkit::action::Origin::Bus)
                .is_err()
        );
        for app in ["global", "ced"] {
            e.ui.row_key = Some(format!("macros:{app}"));
            r.execute("macro.new", &mut e, toolkit::action::Origin::Bus)
                .unwrap();
            assert_eq!(e.ui.dialog, Some(Dialog::NewMacro));
            assert!(
                r.execute("macro.new", &mut e, toolkit::action::Origin::Bus)
                    .is_err()
            );
            e.close_dialog();
        }
        e.ui.row_key = None;
        e.workshop.selected = Some(("ced".into(), "existing".into()));
        r.execute("macro.new", &mut e, toolkit::action::Origin::Bus)
            .unwrap();
        e.close_dialog();
        e.workshop.pending = 1;
        assert!(
            r.execute("macro.new", &mut e, toolkit::action::Origin::Bus)
                .is_err()
        );
        e.workshop.pending = 0;
        e.quitting = true;
        assert!(
            r.execute("macro.new", &mut e, toolkit::action::Origin::Bus)
                .is_err()
        );
    }

    #[test]
    fn traffic_placement_actions_set_and_tick_the_saved_side() {
        let r = registry();
        let mut e = Engine::new(crate::label);
        for (id, side) in [
            ("view.traffic.right", DockSide::Right),
            ("view.traffic.bottom", DockSide::Bottom),
            ("view.traffic.auto", DockSide::Auto),
        ] {
            r.execute(id, &mut e, toolkit::action::Origin::Bus).unwrap();
            assert_eq!(e.ui.dock_side, side);
            assert_eq!(r.checked(id, &e), Some(true));
            assert!(e.dock_requested);
        }
    }

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
    fn view_theme_style_and_framing_choices_set_the_window_choice() {
        let r = registry();
        let mut e = Engine::new(crate::label);
        r.execute("view.style.classic", &mut e, toolkit::action::Origin::Bus)
            .unwrap();
        r.execute("view.decorations.ssd", &mut e, toolkit::action::Origin::Bus)
            .unwrap();
        r.execute("view.captions.left", &mut e, toolkit::action::Origin::Bus)
            .unwrap();
        assert_eq!(e.ui.theme_style, Some(Some(design::Style::Classic)));
        assert_eq!(
            (e.ui.theme_decorations, e.ui.theme_captions),
            (
                Some(design::RequestedDecorations::Server),
                Some(design::CaptionSide::Left)
            )
        );
        assert_eq!(r.checked("view.style.classic", &e), Some(true));
        assert_eq!(r.checked("view.style.own", &e), Some(false));
        r.execute("view.style.own", &mut e, toolkit::action::Origin::Bus)
            .unwrap();
        assert_eq!(e.ui.theme_style, Some(None));
        assert_eq!(r.checked("view.style.own", &e), Some(true));
    }

    #[test]
    fn view_theme_choices_set_the_window_theme_and_tick_it() {
        let r = registry();
        let mut e = Engine::new(crate::label);
        r.execute("view.theme.forest", &mut e, toolkit::action::Origin::Bus)
            .unwrap();
        r.execute("view.mode.dark", &mut e, toolkit::action::Origin::Bus)
            .unwrap();
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
        r.execute("view.mode.session", &mut e, toolkit::action::Origin::Bus)
            .unwrap();
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
                r.execute(id, &mut e, toolkit::action::Origin::Bus).is_err(),
                "{id} must be disabled under a dialog"
            );
        }
    }
}
