// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! inputd's wire contract: the keymap.v1 physical rows, the `input.*` verb
//! names and bodies, and the fixed rule for which strokes are the user's.
//!
//! inputd reads evdev below the compositor, so it is the one place in the
//! stack that sees left and right modifiers as different keys
//! (`KEY_LEFTCTRL` and `KEY_RIGHTCTRL` are distinct codes; xkb merges them).
//! A [`PhysicalBinding`] matches a keycode plus an exact side-specific
//! modifier state.
//!
//! The user keys ([`is_user_key`]) are bare F1–F12 and Right Ctrl, Right Alt
//! or Right Shift with an arrow. They belong to the person: no shipped
//! default binds one, and inputd swallows every one that the person has not
//! bound or marked passthrough. This crate is the single definition of that
//! set.
//!
//! The JSON shapes here are frozen wire constants: a keymap.json written by
//! the earlier broker loads unchanged.

mod action;
mod scope;

use serde::{Deserialize, Serialize};

pub use action::{ActionId, ActionIdError, MAX_ACTION_ID_LEN, MAX_INTERNED_ACTION_IDS};
pub use scope::{BindingScope, RepeatPolicy, ScopeError};

/// The keymap.v1 schema version.
pub const KEYMAP_SCHEMA_VERSION: u32 = 1;

/// The Bus service name inputd registers as.
pub const SERVICE: &str = "inputd";

/// Linux `input-event-codes.h` key codes this contract names.
pub mod codes {
    pub const KEY_P: u16 = 25;
    pub const KEY_UP: u16 = 103;
    pub const KEY_LEFT: u16 = 105;
    pub const KEY_RIGHT: u16 = 106;
    pub const KEY_DOWN: u16 = 108;
    pub const KEY_LEFTCTRL: u16 = 29;
    pub const KEY_RIGHTCTRL: u16 = 97;
    pub const KEY_LEFTSHIFT: u16 = 42;
    pub const KEY_RIGHTSHIFT: u16 = 54;
    pub const KEY_LEFTALT: u16 = 56;
    pub const KEY_RIGHTALT: u16 = 100;
    pub const KEY_LEFTMETA: u16 = 125;
    pub const KEY_RIGHTMETA: u16 = 126;
    /// F1–F10 are 59–68; F11 and F12 are 87 and 88.
    pub const F_KEYS: [u16; 12] = [59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88];
    /// ↑ ← → ↓.
    pub const ARROWS: [u16; 4] = [KEY_UP, KEY_LEFT, KEY_RIGHT, KEY_DOWN];
}

/// Side-specific modifier state: each physical modifier key on its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct SideModifiers {
    pub left_ctrl: bool,
    pub right_ctrl: bool,
    pub left_shift: bool,
    pub right_shift: bool,
    pub left_alt: bool,
    pub right_alt: bool,
    pub left_super: bool,
    pub right_super: bool,
}

impl SideModifiers {
    /// No modifier held.
    pub const NONE: Self = Self {
        left_ctrl: false,
        right_ctrl: false,
        left_shift: false,
        right_shift: false,
        left_alt: false,
        right_alt: false,
        left_super: false,
        right_super: false,
    };
    /// Exactly Right Ctrl.
    pub const RIGHT_CTRL: Self = Self {
        right_ctrl: true,
        ..Self::NONE
    };
    /// Exactly Right Alt.
    pub const RIGHT_ALT: Self = Self {
        right_alt: true,
        ..Self::NONE
    };
    /// Exactly Right Shift.
    pub const RIGHT_SHIFT: Self = Self {
        right_shift: true,
        ..Self::NONE
    };

    /// True when no modifier is held.
    pub const fn is_empty(&self) -> bool {
        !(self.left_ctrl
            || self.right_ctrl
            || self.left_shift
            || self.right_shift
            || self.left_alt
            || self.right_alt
            || self.left_super
            || self.right_super)
    }

    /// Record a modifier key going down or up. Returns false, and changes
    /// nothing, when `code` is not one of the eight modifier keys.
    pub fn set(&mut self, code: u16, down: bool) -> bool {
        use codes::*;
        let slot = match code {
            KEY_LEFTCTRL => &mut self.left_ctrl,
            KEY_RIGHTCTRL => &mut self.right_ctrl,
            KEY_LEFTSHIFT => &mut self.left_shift,
            KEY_RIGHTSHIFT => &mut self.right_shift,
            KEY_LEFTALT => &mut self.left_alt,
            KEY_RIGHTALT => &mut self.right_alt,
            KEY_LEFTMETA => &mut self.left_super,
            KEY_RIGHTMETA => &mut self.right_super,
            _ => return false,
        };
        *slot = down;
        true
    }

    /// True for the eight modifier key codes.
    pub fn is_modifier(code: u16) -> bool {
        let mut probe = Self::NONE;
        probe.set(code, true)
    }
}

/// One physical stroke: a key code with the exact modifier state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhysicalStroke {
    /// The evdev `KEY_*` code.
    pub code: u16,
    #[serde(default)]
    pub modifiers: SideModifiers,
}

/// The fixed user-key rule (Mark, 2026-10-11: "the user's keys belong to
/// the user"):
///
/// - a bare F1–F12, with no modifier at all;
/// - an arrow whose modifiers are some of Right Ctrl, Right Alt and Right
///   Shift, and nothing else.
///
/// Chorded F-keys (`Ctrl+F3`), bare arrows and arrows with any left-hand or
/// Super modifier stay with the apps.
pub fn is_user_key(stroke: &PhysicalStroke) -> bool {
    let mods = stroke.modifiers;
    if codes::F_KEYS.contains(&stroke.code) {
        return mods.is_empty();
    }
    if codes::ARROWS.contains(&stroke.code) {
        let right_hand = SideModifiers {
            right_ctrl: mods.right_ctrl,
            right_alt: mods.right_alt,
            right_shift: mods.right_shift,
            ..SideModifiers::NONE
        };
        return !mods.is_empty() && mods == right_hand;
    }
    false
}

/// Every user key: the twelve bare F-keys, then each arrow with each
/// non-empty combination of the three right-hand modifiers.
pub fn user_keys() -> Vec<PhysicalStroke> {
    let mut out: Vec<PhysicalStroke> = codes::F_KEYS
        .iter()
        .map(|&code| PhysicalStroke {
            code,
            modifiers: SideModifiers::NONE,
        })
        .collect();
    for code in codes::ARROWS {
        for bits in 1u8..8 {
            out.push(PhysicalStroke {
                code,
                modifiers: SideModifiers {
                    right_ctrl: bits & 1 != 0,
                    right_alt: bits & 2 != 0,
                    right_shift: bits & 4 != 0,
                    ..SideModifiers::NONE
                },
            });
        }
    }
    out
}

/// A physical row: a stroke fires a Bus verb.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalBinding {
    pub stroke: PhysicalStroke,
    /// The verb fired on press.
    pub action: ActionId,
    /// The fired verb's body. A JSON object (checked at bind and load);
    /// absent fires an empty body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    /// The registered service the verb is sent to, with `action` as the
    /// command unchanged. Absent: the action's first dot-segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default)]
    pub scope: BindingScope,
    #[serde(default)]
    pub repeat: RepeatPolicy,
    /// Re-emit the stroke unchanged instead of firing. A user key the
    /// person wants an app to see is bound this way.
    #[serde(default)]
    pub passthrough: bool,
}

/// The whole keymap: today only the physical layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputKeymap {
    pub version: u32,
    pub physical: Vec<PhysicalBinding>,
}

impl Default for InputKeymap {
    fn default() -> Self {
        Self {
            version: KEYMAP_SCHEMA_VERSION,
            physical: Vec::new(),
        }
    }
}

/// Interception mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputMode {
    /// Bindings and the user-key rule are in force.
    #[default]
    Normal,
    /// The panic hatch: every stroke re-emitted unchanged, nothing fires.
    Transparent,
}

/// The `input.*` verbs inputd answers. Frozen wire names.
pub mod verbs {
    pub const QUERY: &str = "input.query";
    pub const BIND: &str = "input.bind";
    pub const UNBIND: &str = "input.unbind";
    pub const MODE: &str = "input.mode";
    pub const RELOAD: &str = "input.reload";
    pub const KEY: &str = "input.key";
    pub const POINTER_MOVE: &str = "input.pointer.move";
    pub const POINTER_BUTTON: &str = "input.pointer.button";
    pub const POINTER_SCROLL: &str = "input.pointer.scroll";
}

/// `input.key`: a synthetic key operation, independent of the keymap.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct KeyInjection {
    /// A case-insensitive Linux key name, or a decimal code (0..=0x2ff).
    pub key: String,
    pub action: KeyAction,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyAction {
    Press,
    Release,
    Tap,
}

/// `input.pointer.move`: relative motion in evdev units.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct PointerMove {
    pub dx: i32,
    pub dy: i32,
}

/// `input.pointer.scroll`: wheel steps; positive dy is up, positive dx right.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct PointerScroll {
    pub dy: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dx: Option<i32>,
}

/// `input.pointer.button`.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct PointerButton {
    pub button: PointerButtonName,
    pub action: PointerButtonAction,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PointerButtonName {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PointerButtonAction {
    Press,
    Release,
    Click,
}

/// A keysym row in wire form. inputd parses it so that `input.bind` can
/// refuse it by name; the semantic layer is not resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticRow {
    pub chord: String,
    pub action: ActionId,
    #[serde(default)]
    pub scope: BindingScope,
    #[serde(default)]
    pub repeat: RepeatPolicy,
}

/// The `input.bind` body: a row of either layer, tagged by `layer`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "layer", rename_all = "kebab-case")]
pub enum BindingRow {
    Physical(PhysicalBinding),
    Semantic(SemanticRow),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(code: u16, modifiers: SideModifiers) -> PhysicalStroke {
        PhysicalStroke { code, modifiers }
    }

    #[test]
    fn left_and_right_are_different_modifiers() {
        let mut left = SideModifiers::NONE;
        left.left_ctrl = true;
        assert_ne!(left, SideModifiers::RIGHT_CTRL);
        assert!(SideModifiers::NONE.is_empty());
        assert!(!SideModifiers::RIGHT_CTRL.is_empty());
    }

    #[test]
    fn set_tracks_exactly_the_eight_modifier_keys() {
        let mut mods = SideModifiers::NONE;
        assert!(mods.set(codes::KEY_RIGHTCTRL, true));
        assert_eq!(mods, SideModifiers::RIGHT_CTRL);
        assert!(mods.set(codes::KEY_RIGHTCTRL, false));
        assert!(mods.is_empty());
        assert!(!mods.set(codes::KEY_LEFT, true));
        assert!(mods.is_empty());
        for code in [29, 97, 42, 54, 56, 100, 125, 126] {
            assert!(SideModifiers::is_modifier(code), "{code}");
        }
        assert!(!SideModifiers::is_modifier(codes::KEY_P));
    }

    #[test]
    fn bare_f_keys_are_user_keys_and_chorded_ones_are_not() {
        for code in codes::F_KEYS {
            assert!(is_user_key(&stroke(code, SideModifiers::NONE)), "F {code}");
            for chord in [
                SideModifiers::RIGHT_CTRL,
                SideModifiers::RIGHT_SHIFT,
                SideModifiers {
                    left_ctrl: true,
                    ..SideModifiers::NONE
                },
                SideModifiers {
                    left_shift: true,
                    ..SideModifiers::NONE
                },
            ] {
                assert!(!is_user_key(&stroke(code, chord)), "{code} {chord:?}");
            }
        }
    }

    #[test]
    fn right_hand_arrows_are_user_keys_and_others_are_not() {
        for code in codes::ARROWS {
            for mods in [
                SideModifiers::RIGHT_CTRL,
                SideModifiers::RIGHT_ALT,
                SideModifiers::RIGHT_SHIFT,
                SideModifiers {
                    right_ctrl: true,
                    right_shift: true,
                    ..SideModifiers::NONE
                },
            ] {
                assert!(is_user_key(&stroke(code, mods)), "{code} {mods:?}");
            }
            for mods in [
                SideModifiers::NONE,
                SideModifiers {
                    left_ctrl: true,
                    ..SideModifiers::NONE
                },
                SideModifiers {
                    left_shift: true,
                    ..SideModifiers::NONE
                },
                SideModifiers {
                    left_alt: true,
                    ..SideModifiers::NONE
                },
                SideModifiers {
                    right_ctrl: true,
                    left_shift: true,
                    ..SideModifiers::NONE
                },
                SideModifiers {
                    right_ctrl: true,
                    right_super: true,
                    ..SideModifiers::NONE
                },
            ] {
                assert!(!is_user_key(&stroke(code, mods)), "{code} {mods:?}");
            }
        }
        // Other keys are never user keys, whatever is held.
        assert!(!is_user_key(&stroke(
            codes::KEY_P,
            SideModifiers::RIGHT_CTRL
        )));
        assert!(!is_user_key(&stroke(codes::KEY_P, SideModifiers::NONE)));
    }

    #[test]
    fn user_keys_lists_the_whole_set_once() {
        let all = user_keys();
        assert_eq!(all.len(), 12 + 4 * 7);
        assert!(all.iter().all(is_user_key));
        for (i, s) in all.iter().enumerate() {
            assert!(!all[i + 1..].contains(s), "duplicate {s:?}");
        }
    }

    #[test]
    fn a_legacy_row_round_trips_byte_for_byte() {
        // The exact shape the earlier broker wrote: every modifier spelled
        // out, scope and repeat present, no args or service.
        let legacy = r#"{"stroke":{"code":106,"modifiers":{"left_ctrl":false,"right_ctrl":true,"left_shift":false,"right_shift":false,"left_alt":false,"right_alt":false,"left_super":false,"right_super":false}},"action":"desktop.workspace.next","scope":"global","repeat":"allow","passthrough":false}"#;
        let row: PhysicalBinding = serde_json::from_str(legacy).unwrap();
        assert_eq!(row.stroke.modifiers, SideModifiers::RIGHT_CTRL);
        assert_eq!(serde_json::to_string(&row).unwrap(), legacy);
    }

    #[test]
    fn args_and_service_are_omitted_when_absent() {
        let short = r#"{"stroke":{"code":65},"action":"launch.run"}"#;
        let row: PhysicalBinding = serde_json::from_str(short).unwrap();
        assert_eq!(row.args, None);
        assert_eq!(row.service, None);
        let json = serde_json::to_string(&row).unwrap();
        assert!(
            !json.contains("args") && !json.contains("service"),
            "{json}"
        );
        let targeted = r#"{"stroke":{"code":108,"modifiers":{"right_ctrl":true}},"action":"desktop.clipboard.menu","args":{"x":1},"service":"desktop-vt1"}"#;
        let row: PhysicalBinding = serde_json::from_str(targeted).unwrap();
        assert_eq!(row.service.as_deref(), Some("desktop-vt1"));
        assert_eq!(row.args, Some(serde_json::json!({"x": 1})));
        let back: PhysicalBinding =
            serde_json::from_str(&serde_json::to_string(&row).unwrap()).unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn binding_rows_are_tagged_by_layer() {
        let row: BindingRow = serde_json::from_str(
            r#"{"layer":"physical","stroke":{"code":63},"action":"user.f05"}"#,
        )
        .unwrap();
        assert!(matches!(row, BindingRow::Physical(_)));
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("\"layer\":\"physical\""), "{json}");
        let row: BindingRow = serde_json::from_str(
            r#"{"layer":"semantic","chord":"Ctrl+T","action":"term.tab.new"}"#,
        )
        .unwrap();
        assert!(matches!(row, BindingRow::Semantic(_)));
    }

    #[test]
    fn mode_spellings() {
        assert_eq!(
            serde_json::to_string(&InputMode::Transparent).unwrap(),
            "\"transparent\""
        );
        assert_eq!(
            serde_json::to_string(&InputMode::Normal).unwrap(),
            "\"normal\""
        );
    }
}
