// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! email's commands. The menu bar and the shortcuts come from this
//! registry; there are no Bus verbs of the app's own in this slice (the
//! drive layer's window verbs are described separately).

use egui::{Key, KeyboardShortcut, Modifiers};
use toolkit::{Command, Icon, Registry};

use crate::state::{Mode, State};
use crate::view::UiEvent;

fn idle(state: &State) -> bool {
    state.compose.is_none() && state.ui.dialog.is_none()
}

fn undialogued(state: &State) -> bool {
    state.ui.dialog.is_none()
}

const fn key(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

/// The registry: every command, its menu, its shortcut and its guard.
pub fn registry() -> Registry<State> {
    let mut r = Registry::new();
    r.add(Command {
        id: "file.refresh",
        submenu: None,
        group: 0,
        label: "refresh",
        menu: Some("file"),
        shortcut: key(Modifiers::COMMAND, Key::R),
        icon: Some(Icon::RefreshCw),
        enabled: undialogued,
        run: |state| state.refresh(),
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
        run: State::quit,
    })
    .add(Command {
        id: "mail.compose",
        submenu: None,
        group: 0,
        label: "compose",
        menu: Some("mail"),
        shortcut: key(Modifiers::COMMAND, Key::N),
        icon: Some(Icon::PenSquare),
        enabled: idle,
        run: State::compose,
    })
    .add(Command {
        id: "mail.reply",
        submenu: None,
        group: 1,
        label: "reply",
        menu: Some("mail"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::R),
        icon: Some(Icon::Reply),
        enabled: idle,
        run: State::reply,
    });
    // View › Theme: any scheme, style, mode and framing, in this window only.
    r.theme_menu("view", choice, set_choice);
    r.add(Command {
        id: "view.mail",
        submenu: None,
        group: 0,
        label: "view-mail",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::Digit1),
        icon: Some(Icon::Mail),
        enabled: undialogued,
        run: |state| state.set_mode(Mode::Mail),
    })
    .add(Command {
        id: "view.calendar",
        submenu: None,
        group: 0,
        label: "view-calendar",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::Digit2),
        icon: Some(Icon::Calendar),
        enabled: undialogued,
        run: |state| state.set_mode(Mode::Calendar),
    })
    .add(Command {
        id: "view.contacts",
        submenu: None,
        group: 0,
        label: "view-contacts",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::Digit3),
        icon: Some(Icon::Contact),
        enabled: undialogued,
        run: |state| state.set_mode(Mode::Contacts),
    })
    .add(Command {
        id: "help.shortcuts",
        submenu: None,
        group: 0,
        label: "shortcuts",
        menu: Some("help"),
        shortcut: None,
        icon: None,
        enabled: undialogued,
        run: |state| state.open_dialog(crate::state::Dialog::Shortcuts),
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
        run: |state| state.open_dialog(crate::state::Dialog::About),
    })
    // The title bar's right-hand controls (no menu of their own).
    .add(Command {
        id: "view.mode",
        submenu: None,
        group: 0,
        label: "toggle-mode",
        menu: None,
        shortcut: None,
        icon: None,
        enabled: toolkit::command::always,
        run: State::toggle_mode,
    });
    r
}

/// The window's theme choice, as the Theme menu reads it.
pub fn choice(state: &State) -> toolkit::theme_menu::Choice {
    toolkit::theme_menu::Choice {
        scheme: state.ui.theme.scheme,
        style: state.ui.theme.style,
        mode: state.ui.theme.mode,
        decorations: state.ui.theme.decorations,
        captions: state.ui.theme.captions,
    }
}

/// Store the Theme menu's choice as the window's.
fn set_choice(state: &mut State, choice: toolkit::theme_menu::Choice) {
    state.ui.theme.scheme = choice.scheme;
    state.ui.theme.style = choice.style;
    state.ui.theme.mode = choice.mode;
    state.ui.theme.decorations = choice.decorations;
    state.ui.theme.captions = choice.captions;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every label is in the catalogue.
    #[test]
    fn every_label_is_in_the_catalogue() {
        let strings = crate::strings();
        let known = |key: &str| toolkit::command::label_text(&strings, key) != key;
        for command in registry().iter() {
            assert!(
                known(command.label),
                "{} label {:?}",
                command.id,
                command.label
            );
        }
    }
}
