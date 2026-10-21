// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Synthetic key and pointer input for `input.key` and `input.pointer.*`,
//! through a uinput device of its own, independent of any grab or reader.

use std::io;
use std::time::Duration;

use inputd_schema::{
    KeyAction, PointerButton, PointerButtonAction, PointerButtonName, PointerMove, PointerScroll,
    codes,
};

use crate::devices::{EV_REL, EVENT_SIZE, KEY_MAX, UinputDevice, encode};
use crate::grab::{EV_KEY, EV_SYN, SYN_REPORT};

const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_HWHEEL: u16 = 0x06;
const REL_WHEEL: u16 = 0x08;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;

/// A Linux physical key name (case-insensitive, `KEY_` optional), or a
/// decimal code. Names are physical keys, not layout keysyms.
pub fn key_code(key: &str) -> Result<u16, String> {
    if !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) {
        return key
            .parse::<u16>()
            .ok()
            .filter(|&code| code <= KEY_MAX)
            .ok_or_else(|| format!("invalid key code {key:?}: expected 0..={KEY_MAX}"));
    }
    let name = key.to_ascii_lowercase();
    let name = name.strip_prefix("key_").unwrap_or(&name);
    KEY_NAMES
        .iter()
        .find_map(|&(candidate, code)| (candidate == name).then_some(code))
        .ok_or_else(|| format!("unknown key: {key:?}"))
}

// input-event-codes.h; letter codes follow the physical rows.
const KEY_NAMES: &[(&str, u16)] = &[
    ("f1", 59),
    ("f2", 60),
    ("f3", 61),
    ("f4", 62),
    ("f5", 63),
    ("f6", 64),
    ("f7", 65),
    ("f8", 66),
    ("f9", 67),
    ("f10", 68),
    ("f11", 87),
    ("f12", 88),
    ("a", 30),
    ("b", 48),
    ("c", 46),
    ("d", 32),
    ("e", 18),
    ("f", 33),
    ("g", 34),
    ("h", 35),
    ("i", 23),
    ("j", 36),
    ("k", 37),
    ("l", 38),
    ("m", 50),
    ("n", 49),
    ("o", 24),
    ("p", 25),
    ("q", 16),
    ("r", 19),
    ("s", 31),
    ("t", 20),
    ("u", 22),
    ("v", 47),
    ("w", 17),
    ("x", 45),
    ("y", 21),
    ("z", 44),
    ("0", 11),
    ("1", 2),
    ("2", 3),
    ("3", 4),
    ("4", 5),
    ("5", 6),
    ("6", 7),
    ("7", 8),
    ("8", 9),
    ("9", 10),
    ("enter", 28),
    ("escape", 1),
    ("space", 57),
    ("tab", 15),
    ("backspace", 14),
    ("delete", 111),
    ("left", codes::KEY_LEFT),
    ("right", codes::KEY_RIGHT),
    ("up", codes::KEY_UP),
    ("down", codes::KEY_DOWN),
    ("home", 102),
    ("end", 107),
    ("pageup", 104),
    ("pagedown", 109),
    ("leftctrl", codes::KEY_LEFTCTRL),
    ("rightctrl", codes::KEY_RIGHTCTRL),
    ("leftshift", codes::KEY_LEFTSHIFT),
    ("rightshift", codes::KEY_RIGHTSHIFT),
    ("leftalt", codes::KEY_LEFTALT),
    ("rightalt", codes::KEY_RIGHTALT),
    ("leftmeta", codes::KEY_LEFTMETA),
    ("rightmeta", codes::KEY_RIGHTMETA),
];

/// The service's injector. Its device is made on the first valid request
/// and kept for the process; an invalid request never creates it.
#[derive(Default)]
pub struct Injector {
    device: Option<UinputDevice>,
}

impl Injector {
    pub fn inject(&mut self, request: Injection) -> io::Result<()> {
        if self.device.is_none() {
            self.device = Some(UinputDevice::pointer()?);
            // Give the compositor time to find the device before the first
            // frame.
            std::thread::sleep(Duration::from_millis(300));
        }
        let device = self.device.as_mut().expect("injector device created");
        for event in request.events() {
            device.write_event(&event)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn with_test_file(file: std::fs::File) -> Self {
        Self {
            device: Some(UinputDevice::with_test_file(file)),
        }
    }
}

pub enum Injection {
    Key { code: u16, action: KeyAction },
    Move(PointerMove),
    Button(PointerButton),
    Scroll(PointerScroll),
}

impl Injection {
    fn events(self) -> Vec<[u8; EVENT_SIZE]> {
        let mut events = Vec::with_capacity(4);
        let mut emit = |kind, code, value| events.push(encode(kind, code, value));
        let mut press_release = |code: u16, press: bool, release: bool| {
            if press {
                emit(EV_KEY, code, 1);
            }
            if press && release {
                emit(EV_SYN, SYN_REPORT, 0);
            }
            if release {
                emit(EV_KEY, code, 0);
            }
        };
        match self {
            Self::Key { code, action } => match action {
                KeyAction::Press => press_release(code, true, false),
                KeyAction::Release => press_release(code, false, true),
                KeyAction::Tap => press_release(code, true, true),
            },
            Self::Button(PointerButton { button, action }) => {
                let code = match button {
                    PointerButtonName::Left => BTN_LEFT,
                    PointerButtonName::Right => BTN_RIGHT,
                    PointerButtonName::Middle => BTN_MIDDLE,
                };
                match action {
                    PointerButtonAction::Press => press_release(code, true, false),
                    PointerButtonAction::Release => press_release(code, false, true),
                    PointerButtonAction::Click => press_release(code, true, true),
                }
            }
            Self::Move(PointerMove { dx, dy }) => {
                emit(EV_REL, REL_X, dx);
                emit(EV_REL, REL_Y, dy);
            }
            Self::Scroll(PointerScroll { dy, dx }) => {
                emit(EV_REL, REL_WHEEL, dy);
                if let Some(dx) = dx {
                    emit(EV_REL, REL_HWHEEL, dx);
                }
            }
        }
        events.push(encode(EV_SYN, SYN_REPORT, 0));
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_key_names_and_raw_codes() {
        for (name, code) in [
            ("F1", 59),
            ("F10", 68),
            ("F11", 87),
            ("F12", 88),
            ("Enter", 28),
            ("Escape", 1),
            ("Space", 57),
            ("Tab", 15),
            ("Backspace", 14),
            ("Delete", 111),
            ("Left", 105),
            ("Right", 106),
            ("Up", 103),
            ("Down", 108),
            ("Home", 102),
            ("End", 107),
            ("PageUp", 104),
            ("PageDown", 109),
            ("LeftCtrl", 29),
            ("RightCtrl", 97),
            ("LeftShift", 42),
            ("RightShift", 54),
            ("LeftAlt", 56),
            ("RightAlt", 100),
            ("LeftMeta", 125),
            ("RightMeta", 126),
        ] {
            assert_eq!(key_code(name), Ok(code));
            assert_eq!(key_code(&name.to_ascii_lowercase()), Ok(code));
            assert_eq!(key_code(&name.to_ascii_uppercase()), Ok(code));
        }
        for (name, code) in ('a'..='z').zip([
            30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47,
            17, 45, 21, 44,
        ]) {
            assert_eq!(key_code(&name.to_string()), Ok(code));
            assert_eq!(key_code(&name.to_ascii_uppercase().to_string()), Ok(code));
        }
        for (name, code) in (0..=9).zip([11, 2, 3, 4, 5, 6, 7, 8, 9, 10]) {
            assert_eq!(key_code(&format!("KEY_{name}")), Ok(code));
        }
        for code in 0..=KEY_MAX {
            assert_eq!(key_code(&code.to_string()), Ok(code));
        }
        assert_eq!(key_code("0067"), Ok(67));
        assert!(key_code("NotAKey").unwrap_err().contains("NotAKey"));
        assert!(key_code("768").is_err());
    }
}
