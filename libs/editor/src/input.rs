// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keys the editor handles itself: navigation, editing keys, and the
//! standard Select All, Undo and Redo chords when nothing else claimed them.
//! Typed text arrives separately, as text events.

use egui::{Key, Modifiers};

use crate::{EditCommand, Event, Motion};

/// What a key press does, if the editor handles it. `page` is the number of
/// full rows (the Page Up and Page Down distance).
pub fn key(key: Key, mods: Modifiers, page: usize) -> Option<Event> {
    let extend = mods.shift;
    let word = mods.command;
    let mv = |to| Some(Event::Command(EditCommand::Move { to, extend }));
    let cmd = |c| Some(Event::Command(c));
    match key {
        Key::ArrowLeft => mv(if word { Motion::WordLeft } else { Motion::Left }),
        Key::ArrowRight => mv(if word {
            Motion::WordRight
        } else {
            Motion::Right
        }),
        Key::ArrowUp if !word => mv(Motion::Up),
        Key::ArrowDown if !word => mv(Motion::Down),
        Key::Home => mv(if word { Motion::DocStart } else { Motion::Home }),
        Key::End => mv(if word { Motion::DocEnd } else { Motion::End }),
        Key::PageUp if !word => mv(Motion::PageUp(page)),
        Key::PageDown if !word => mv(Motion::PageDown(page)),
        Key::Enter if !word && !mods.alt => cmd(EditCommand::Newline),
        Key::Backspace if !mods.alt => cmd(if word {
            EditCommand::DeleteWordLeft
        } else {
            EditCommand::Backspace
        }),
        Key::Delete if !mods.alt && !mods.shift => cmd(if word {
            EditCommand::DeleteWordRight
        } else {
            EditCommand::Delete
        }),
        Key::Tab if !word && !mods.alt => cmd(if extend {
            EditCommand::Outdent
        } else {
            EditCommand::Tab
        }),
        Key::A if word && !mods.shift && !mods.alt => cmd(EditCommand::SelectAll),
        Key::Z if word && !mods.alt => Some(if mods.shift { Event::Redo } else { Event::Undo }),
        Key::Y if word && !mods.alt && !mods.shift => Some(Event::Redo),
        _ => None,
    }
}

/// Typed text, unless it is empty or carries control characters (those are
/// keys, not text).
pub fn text(s: &str) -> Option<Event> {
    (!s.is_empty() && !s.chars().any(char::is_control))
        .then(|| Event::Command(EditCommand::Insert(s.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(to: Motion, extend: bool) -> Option<Event> {
        Some(Event::Command(EditCommand::Move { to, extend }))
    }

    #[test]
    fn navigation_and_editing_keys() {
        let none = Modifiers::NONE;
        assert_eq!(key(Key::ArrowLeft, none, 30), mv(Motion::Left, false));
        assert_eq!(
            key(Key::ArrowRight, Modifiers::COMMAND | Modifiers::SHIFT, 30),
            mv(Motion::WordRight, true)
        );
        assert_eq!(
            key(Key::Home, Modifiers::COMMAND, 30),
            mv(Motion::DocStart, false)
        );
        assert_eq!(
            key(Key::PageDown, none, 30),
            mv(Motion::PageDown(30), false)
        );
        assert_eq!(
            key(Key::Backspace, Modifiers::COMMAND, 30),
            Some(Event::Command(EditCommand::DeleteWordLeft))
        );
        assert_eq!(
            key(Key::Tab, Modifiers::SHIFT, 30),
            Some(Event::Command(EditCommand::Outdent))
        );
        assert_eq!(
            key(Key::Enter, none, 30),
            Some(Event::Command(EditCommand::Newline))
        );
        assert_eq!(
            key(Key::Escape, none, 30),
            None,
            "Escape belongs to the application"
        );
        assert_eq!(
            key(Key::Delete, Modifiers::SHIFT, 30),
            None,
            "Shift+Delete is Cut"
        );
    }

    #[test]
    fn standard_chords() {
        assert_eq!(key(Key::Z, Modifiers::COMMAND, 1), Some(Event::Undo));
        assert_eq!(
            key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT, 1),
            Some(Event::Redo)
        );
        assert_eq!(key(Key::Y, Modifiers::COMMAND, 1), Some(Event::Redo));
        assert_eq!(
            key(Key::A, Modifiers::COMMAND, 1),
            Some(Event::Command(EditCommand::SelectAll))
        );
        assert_eq!(key(Key::Z, Modifiers::NONE, 1), None, "a letter is text");
    }

    #[test]
    fn text_input() {
        assert_eq!(
            text("a"),
            Some(Event::Command(EditCommand::Insert("a".into())))
        );
        assert_eq!(text("\u{8}"), None, "control characters are keys");
        assert_eq!(text(""), None);
    }
}
