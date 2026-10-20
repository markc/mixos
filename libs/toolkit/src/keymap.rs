// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Chord text, the one grammar every app's key bindings use: `Ctrl`, `Alt`
//! and `Shift` (any order) then one key, joined by `+`. `Ctrl` is the
//! platform's command key. Keys are egui's names (`ArrowLeft`, `PageDown`,
//! `F6`, `Delete`), one letter or digit, or a short form: `Up`, `Down`,
//! `Left`, `Right`, `=`, `-`, `/`.
//!
//! The person's bindings come from the desktop profile
//! (`settings::keymap`); [`crate::Registry::rebind`] applies them.

pub use ::command::keymap::Overrides;
use egui::{Key, KeyboardShortcut, Modifiers};

/// The shortcut `text` names, or `None` when it is not a chord egui has.
pub fn parse_chord(text: &str) -> Option<KeyboardShortcut> {
    let parts: Vec<&str> = text.split('+').collect();
    let (key, mods) = parts.split_last()?;
    let mut modifiers = Modifiers::NONE;
    for m in mods {
        let add = match *m {
            "Ctrl" => Modifiers::COMMAND,
            "Alt" => Modifiers::ALT,
            "Shift" => Modifiers::SHIFT,
            _ => return None,
        };
        if modifiers.contains(add) {
            return None;
        }
        modifiers = modifiers.plus(add);
    }
    let key = match *key {
        "Up" => Key::ArrowUp,
        "Down" => Key::ArrowDown,
        "Left" => Key::ArrowLeft,
        "Right" => Key::ArrowRight,
        "=" => Key::Equals,
        "-" => Key::Minus,
        "/" => Key::Slash,
        "" => return None,
        one if one.chars().count() == 1 => Key::from_name(&one.to_ascii_uppercase())?,
        name => Key::from_name(name)?,
    };
    Some(KeyboardShortcut::new(modifiers, key))
}

/// The text `shortcut` is stored and reported as; [`parse_chord`] reads it
/// back.
pub fn chord_text(shortcut: &KeyboardShortcut) -> String {
    let mut out = String::new();
    let m = shortcut.modifiers;
    for (on, name) in [
        (m.command || m.ctrl || m.mac_cmd, "Ctrl+"),
        (m.alt, "Alt+"),
        (m.shift, "Shift+"),
    ] {
        if on {
            out.push_str(name);
        }
    }
    // egui prints arrows as symbols; the grammar names them.
    out.push_str(match shortcut.logical_key {
        Key::ArrowLeft => "ArrowLeft",
        Key::ArrowRight => "ArrowRight",
        Key::ArrowUp => "ArrowUp",
        Key::ArrowDown => "ArrowDown",
        Key::Equals => "=",
        Key::Minus => "-",
        Key::Slash => "/",
        key => key.name(),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_parse_and_round_trip() {
        for text in [
            "F2",
            "Ctrl+B",
            "Alt+ArrowLeft",
            "Ctrl+Shift+N",
            "Ctrl+Alt+1",
            "Ctrl+=",
            "Ctrl+-",
            "Ctrl+/",
            "Shift+Delete",
            "Ctrl+PageDown",
            "Backspace",
        ] {
            let s = parse_chord(text).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(chord_text(&s), text, "round trip");
        }
        assert_eq!(
            parse_chord("Shift+Ctrl+n"),
            Some(KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT,
                Key::N
            ))
        );
        assert_eq!(
            parse_chord("Ctrl+Shift+Up"),
            parse_chord("Ctrl+Shift+ArrowUp")
        );
        assert_eq!(parse_chord("Ctrl+1").unwrap().logical_key, Key::Num1);
        for bad in ["", "Ctrl+", "Hyper+X", "Ctrl+Ctrl+B", "Ctrl+Nope"] {
            assert!(parse_chord(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn every_egui_key_round_trips_through_the_adapter_cache() {
        for &key in Key::ALL {
            let shortcut = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, key);
            let text = chord_text(&shortcut);
            assert_eq!(parse_chord(&text), Some(shortcut), "{text}");
        }
    }
}
