// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Term's commands: one per action, so the menus, the keyboard, the
//! toolkit's Bus drive and `term.execute` all reach the same path, and each
//! run is published on `term.action.executed`.
//!
//! The chords shown are the terminal's own (`input.rs` decides them, with
//! the layout fallbacks and autorepeat rules a terminal needs); the
//! keyboard dispatcher runs these commands by id.

use crate::app::App;
use crate::input::Action;
use egui::{Key, KeyboardShortcut, Modifiers};
use term_core::panes::{Direction, SplitDir};
use term_core::terminal::ScrollRequest;
use toolkit::{Registry, action::always};

fn chord(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

fn ctrl_shift() -> Modifiers {
    Modifiers::CTRL | Modifiers::SHIFT
}

macro_rules! commands {
    ($r:ident; $( $id:literal, $label:literal, $menu:literal, $group:literal, $shortcut:expr, $enabled:expr, $action:expr; )*) => {
        $(
            $r.add(toolkit::Action {
                id: $id,
                label: $label,
                menu: Some($menu),
                submenu: None,
                group: $group,
                shortcut: $shortcut,
                icon: None,
                enabled: $enabled,
                run: |a: &mut App| a.act($action),
            });
        )*
    };
}

fn running(a: &App) -> bool {
    a.running()
}

fn split(a: &App) -> bool {
    a.split()
}

fn several_tabs(a: &App) -> bool {
    a.several_tabs()
}

pub fn registry() -> Registry<App> {
    let mut r = Registry::new();
    r.add(toolkit::macros::command(always, |a: &mut App| {
        a.macros_open = !a.macros_open;
    }));
    let cs = ctrl_shift();
    let (ctrl, shift) = (Modifiers::CTRL, Modifiers::SHIFT);
    commands! { r;
        "tab.new", "tab-new", "file", 0, chord(cs, Key::T), running, Action::NewTab;
    }
    // Targeted: the strip's close box and `term.execute {"id":"tab.close",
    // "tab":N}` name a tab; with none it is the active one.
    r.add(toolkit::Action {
        id: "tab.close",
        label: "tab-close",
        menu: Some("file"),
        submenu: None,
        group: 0,
        shortcut: chord(cs, Key::W),
        icon: None,
        enabled: running,
        run: App::close_target_tab,
    });
    commands! { r;
        "app.quit", "app-quit", "file", 1, chord(cs, Key::Q), always, Action::Quit;
        "edit.copy", "edit-copy", "edit", 0, chord(cs, Key::C), running, Action::Copy;
        "edit.paste", "edit-paste", "edit", 0, chord(cs, Key::V), running, Action::Paste;
        "view.zoom_in", "view-zoom-in", "view", 0, chord(ctrl, Key::Equals), always, Action::FontIncrease;
        "view.zoom_out", "view-zoom-out", "view", 0, chord(ctrl, Key::Minus), always, Action::FontDecrease;
        "view.zoom_reset", "view-zoom-reset", "view", 0, chord(ctrl, Key::Num0), always, Action::FontReset;
        "scroll.page_up", "scroll-page-up", "view", 1, chord(shift, Key::PageUp), running, Action::Scroll(ScrollRequest::PageUp);
        "scroll.page_down", "scroll-page-down", "view", 1, chord(shift, Key::PageDown), running, Action::Scroll(ScrollRequest::PageDown);
        "scroll.top", "scroll-top", "view", 1, chord(shift, Key::Home), running, Action::Scroll(ScrollRequest::Top);
        "scroll.bottom", "scroll-bottom", "view", 1, chord(shift, Key::End), running, Action::Scroll(ScrollRequest::Bottom);
        "pane.split_right", "pane-split-right", "pane", 0, chord(cs, Key::E), running, Action::Split(SplitDir::Vertical);
        "pane.split_down", "pane-split-down", "pane", 0, chord(cs, Key::O), running, Action::Split(SplitDir::Horizontal);
        "pane.close", "pane-close", "pane", 0, chord(cs, Key::X), running, Action::ClosePane;
        "pane.next", "pane-next", "pane", 1, chord(ctrl, Key::Tab), split, Action::CyclePane { forward: true };
        "pane.prev", "pane-prev", "pane", 1, chord(cs, Key::Tab), split, Action::CyclePane { forward: false };
        "pane.focus_left", "pane-focus-left", "pane", 2, chord(cs, Key::ArrowLeft), split, Action::Focus(Direction::Left);
        "pane.focus_right", "pane-focus-right", "pane", 2, chord(cs, Key::ArrowRight), split, Action::Focus(Direction::Right);
        "pane.focus_up", "pane-focus-up", "pane", 2, chord(cs, Key::ArrowUp), split, Action::Focus(Direction::Up);
        "pane.focus_down", "pane-focus-down", "pane", 2, chord(cs, Key::ArrowDown), split, Action::Focus(Direction::Down);
        "tab.next", "tab-next", "tabs", 0, chord(ctrl, Key::PageDown), several_tabs, Action::Cycle { forward: true };
        "tab.prev", "tab-prev", "tabs", 0, chord(ctrl, Key::PageUp), several_tabs, Action::Cycle { forward: false };
    }
    // Palette and Bus only: the strip's tabs and a click on a pane.
    r.add(toolkit::Action {
        id: "tab.select",
        label: "tab-select",
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: running,
        run: App::select_target_tab,
    });
    r.add(toolkit::Action {
        id: "pane.focus",
        label: "pane-focus",
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: running,
        run: App::focus_target_pane,
    });
    r.add(toolkit::Action {
        id: "help.keys",
        label: "help-keys",
        menu: Some("help"),
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: always,
        run: |a: &mut App| a.keys_dialog = true,
    });
    r.add(toolkit::Action {
        id: "help.about",
        label: "help-about",
        menu: Some("help"),
        submenu: None,
        group: 1,
        shortcut: None,
        icon: None,
        enabled: always,
        run: |a: &mut App| a.about_dialog = true,
    });
    // View › Theme: any scheme, style, mode and framing, in this window only.
    r.theme_menu("view", |a: &App| a.theme, |a: &mut App, c| a.theme = c);
    r.search_action(toolkit::search::command(toolkit::action::always, |a: &mut App| a.search_open = true));
    r.search_menu("help");
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_a_label_in_the_catalogue() {
        let strings = crate::strings();
        for command in registry().iter() {
            assert!(
                strings.has(command.label)
                    || toolkit::action::label_text(&strings, command.label) != command.label,
                "{} has no label {}",
                command.id,
                command.label
            );
            if let Some(menu) = command.menu {
                assert!(
                    strings.has(menu) || toolkit::action::label_text(&strings, menu) != menu,
                    "{} menu {menu}",
                    command.id
                );
            }
        }
    }

    /// The accelerators shown in the menus are the keys the dispatcher
    /// answers (`input::action_for` with the command the action names).
    #[test]
    fn the_menu_chords_are_the_keys_that_run_them() {
        for command in registry().iter() {
            let Some(shortcut) = command.shortcut else {
                continue;
            };
            let action = crate::input::action_for(shortcut.logical_key, shortcut.modifiers)
                .unwrap_or_else(|| panic!("{} shows a chord nothing answers", command.id));
            assert_eq!(action.command(), command.id);
        }
    }
}
