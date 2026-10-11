// SPDX-License-Identifier: MIT OR Apache-2.0
//! dopus's key bindings: the default chords, and the user's overlay
//! `keymap.conf.mix` in the app's config directory.
//!
//! The overlay keeps the shape the iced dopus read: `custom` is a list of
//! `{action, chord}` entries. Any entry for an action replaces all of that
//! action's default chords; `chord: nil` leaves it unbound, and several
//! entries give it several chords. A chord is a list of strokes; only
//! single-stroke chords are bound (no default uses more), and a longer one
//! is reported and skipped. Other overlay fields (`version`,
//! `chord_timeout_ms`, `scope`, `repeat`, `allow_in_editable`) are accepted
//! and ignored.

use std::path::Path;

use egui::{Key, KeyboardShortcut, Modifiers};
use serde::Deserialize;

/// The default chords, `(chord, action)`, in the order menus show them: an
/// action's first chord is its menu accelerator.
pub const DEFAULT: &[(&str, &str)] = &[
    ("Ctrl+B", "view.toggle-places"),
    ("Ctrl+I", "view.toggle-properties"),
    ("Ctrl+L", "location.focus"),
    ("Enter", "file.open"),
    ("Ctrl+Shift+N", "file.new-folder"),
    ("F2", "file.rename"),
    ("Ctrl+E", "file.rename"),
    ("Ctrl+C", "file.copy-other-pane"),
    ("Ctrl+X", "file.move-other-pane"),
    ("Delete", "file.delete"),
    ("Ctrl+Q", "app.quit"),
    ("Alt+ArrowLeft", "nav.back"),
    ("Alt+ArrowRight", "nav.forward"),
    ("Backspace", "nav.parent"),
    ("Alt+Home", "nav.home"),
    ("F6", "nav.switch-pane"),
    ("Tab", "nav.switch-pane"),
    ("F5", "view.refresh"),
    ("Ctrl+R", "view.refresh"),
    ("Ctrl+H", "view.toggle-hidden"),
    ("Ctrl+1", "view.sort-name"),
    ("Ctrl+2", "view.sort-size"),
    ("Ctrl+3", "view.sort-modified"),
    ("ArrowDown", "selection.next"),
    ("ArrowUp", "selection.previous"),
    ("Home", "selection.first"),
    ("End", "selection.last"),
    ("Ctrl+Alt+D", "theme.mode-toggle"),
    ("Ctrl+Alt+1", "theme.scheme-ocean"),
    ("Ctrl+Alt+2", "theme.scheme-crimson"),
    ("Ctrl+Alt+3", "theme.scheme-stone"),
    ("Ctrl+Alt+4", "theme.scheme-forest"),
    ("Ctrl+Alt+5", "theme.scheme-sunset"),
    ("Ctrl+Alt+6", "theme.scheme-mono"),
];

/// One parsed stroke: modifiers and a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

impl Chord {
    /// The egui shortcut for this stroke (Ctrl is the command key).
    pub fn shortcut(self) -> KeyboardShortcut {
        let mut modifiers = Modifiers::NONE;
        if self.ctrl {
            modifiers = modifiers.plus(Modifiers::COMMAND);
        }
        if self.alt {
            modifiers = modifiers.plus(Modifiers::ALT);
        }
        if self.shift {
            modifiers = modifiers.plus(Modifiers::SHIFT);
        }
        KeyboardShortcut::new(modifiers, self.key)
    }
}

/// Parse `"Ctrl+Shift+N"`-style text: modifiers in any order, the key last.
/// Keys are egui's names (`ArrowLeft`, `F6`, `Delete`) or one letter or
/// digit.
pub fn parse_chord(text: &str) -> Option<Chord> {
    let parts: Vec<&str> = text.split('+').collect();
    let (key, mods) = parts.split_last()?;
    let mut chord = Chord {
        ctrl: false,
        alt: false,
        shift: false,
        key: Key::A,
    };
    for m in mods {
        match *m {
            "Ctrl" => chord.ctrl = true,
            "Alt" => chord.alt = true,
            "Shift" => chord.shift = true,
            _ => return None,
        }
    }
    chord.key = if key.chars().count() == 1 {
        Key::from_name(&key.to_ascii_uppercase())?
    } else {
        Key::from_name(key)?
    };
    Some(chord)
}

/// The text a chord is shown and reported as.
pub fn chord_text(chord: Chord) -> String {
    let mut out = String::new();
    for (on, name) in [
        (chord.ctrl, "Ctrl+"),
        (chord.alt, "Alt+"),
        (chord.shift, "Shift+"),
    ] {
        if on {
            out.push_str(name);
        }
    }
    // The keymap names the arrows as egui parses them, not as it prints them.
    out.push_str(match chord.key {
        Key::ArrowLeft => "ArrowLeft",
        Key::ArrowRight => "ArrowRight",
        Key::ArrowUp => "ArrowUp",
        Key::ArrowDown => "ArrowDown",
        key => key.name(),
    });
    out
}

/// The effective bindings, `(chord, action)`, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    pub bindings: Vec<(Chord, String)>,
    /// What the overlay held that could not be bound.
    pub notes: Vec<String>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: DEFAULT
                .iter()
                .map(|(c, a)| {
                    (
                        parse_chord(c).expect("a default chord parses"),
                        (*a).to_owned(),
                    )
                })
                .collect(),
            notes: Vec::new(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Overlay {
    #[serde(default)]
    custom: Vec<Override>,
}

#[derive(Debug, Deserialize)]
struct Override {
    action: String,
    #[serde(default)]
    chord: Option<Vec<String>>,
}

impl Keymap {
    /// Every chord bound to `action`, in order.
    pub fn chords_for(&self, action: &str) -> Vec<Chord> {
        self.bindings
            .iter()
            .filter(|(_, a)| a == action)
            .map(|(c, _)| *c)
            .collect()
    }

    /// Apply `overlay`: each overridden action loses its defaults and takes
    /// the overlay's single-stroke chords. A chord now bound twice keeps
    /// only the overlay's binding.
    fn apply(&mut self, overlay: Overlay) {
        let replaced: std::collections::BTreeSet<&str> =
            overlay.custom.iter().map(|o| o.action.as_str()).collect();
        self.bindings
            .retain(|(_, a)| !replaced.contains(a.as_str()));
        let mut added = Vec::new();
        for o in &overlay.custom {
            let Some(strokes) = &o.chord else { continue };
            match strokes.as_slice() {
                [one] => match parse_chord(one) {
                    Some(chord) => added.push((chord, o.action.clone())),
                    None => self.notes.push(format!(
                        "{}: chord {one:?} is not a key dopus knows",
                        o.action
                    )),
                },
                _ => self.notes.push(format!(
                    "{}: chord {strokes:?} has {} strokes; only single strokes are bound",
                    o.action,
                    strokes.len()
                )),
            }
        }
        self.bindings
            .retain(|(c, _)| !added.iter().any(|(d, _)| d == c));
        self.bindings.extend(added);
    }
}

/// The effective keymap: the defaults with the overlay at `path` (if any)
/// applied. An overlay that cannot be read is an error; one that binds a
/// chord dopus cannot use leaves a note.
pub fn load(path: Option<&Path>) -> Result<Keymap, String> {
    let mut keymap = Keymap::default();
    let Some(path) = path.filter(|p| p.exists()) else {
        return Ok(keymap);
    };
    let overlay: Overlay = config::store::load_conf_mix_path(path)
        .map_err(|e| format!("keymap overlay {}: {e}", path.display()))?;
    keymap.apply(overlay);
    Ok(keymap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_parses_once() {
        let mut seen = std::collections::HashSet::new();
        for (text, action) in DEFAULT {
            let c = parse_chord(text).unwrap_or_else(|| panic!("{text} does not parse"));
            assert!(seen.insert(c), "{text} bound twice (again to {action})");
            assert_eq!(&chord_text(c), text, "round trip");
        }
    }

    #[test]
    fn chords_become_shortcuts() {
        let c = parse_chord("Ctrl+Shift+N").unwrap();
        assert_eq!(
            c.shortcut(),
            KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::N)
        );
        assert_eq!(parse_chord("Alt+ArrowLeft").unwrap().key, Key::ArrowLeft);
        assert_eq!(parse_chord("Ctrl+1").unwrap().key, Key::Num1);
        assert!(parse_chord("Hyper+X").is_none());
        assert!(parse_chord("Ctrl+Nope").is_none());
    }

    #[test]
    fn an_overlay_replaces_unbinds_and_adds() {
        let mut k = Keymap::default();
        k.apply(Overlay {
            custom: vec![
                Override {
                    action: "file.rename".into(),
                    chord: Some(vec!["Ctrl+M".into()]),
                },
                Override {
                    action: "file.delete".into(),
                    chord: None,
                },
                Override {
                    action: "view.refresh".into(),
                    chord: Some(vec!["Ctrl+K".into(), "R".into()]),
                },
                // Takes Ctrl+B from Places.
                Override {
                    action: "nav.home".into(),
                    chord: Some(vec!["Ctrl+B".into()]),
                },
            ],
        });
        assert_eq!(
            k.chords_for("file.rename"),
            vec![parse_chord("Ctrl+M").unwrap()]
        );
        assert!(k.chords_for("file.delete").is_empty());
        assert!(k.chords_for("view.refresh").is_empty());
        assert_eq!(k.notes.len(), 1, "{:?}", k.notes);
        assert!(k.chords_for("view.toggle-places").is_empty());
        assert_eq!(
            k.chords_for("nav.home"),
            vec![parse_chord("Ctrl+B").unwrap()]
        );
    }

    #[test]
    fn the_overlay_file_loads_in_the_iced_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keymap.conf.mix");
        std::fs::write(
            &path,
            "{\n  version: 1,\n  chord_timeout_ms: 800,\n  custom: [\n    { action: \"file.delete\", chord: [\"Shift+Delete\"], scope: \"global\" },\n    { action: \"nav.parent\", chord: nil }\n  ]\n}\n",
        )
        .unwrap();
        let k = load(Some(&path)).unwrap();
        assert_eq!(
            k.chords_for("file.delete"),
            vec![parse_chord("Shift+Delete").unwrap()]
        );
        assert!(k.chords_for("nav.parent").is_empty());
        assert_eq!(
            load(Some(&dir.path().join("absent"))).unwrap(),
            Keymap::default()
        );
    }
}
