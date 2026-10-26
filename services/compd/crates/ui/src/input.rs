// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Input for compositor-owned UIs.
//!
//! The compositor speaks in what it has (surface-local points, Linux button
//! codes, xkb keysyms with their composed text, discrete or continuous axis
//! deltas) through [`Input`]. Each runtime turns that into egui events with
//! its [`Translator`], which remembers the pointer position and the modifier
//! state egui wants on every button, key and wheel event. Nothing outside this
//! crate touches egui's event types.

/// Held modifier keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
        logo: false,
    };
    pub const SHIFT: Self = Self {
        shift: true,
        ..Self::NONE
    };
    pub const CTRL: Self = Self {
        ctrl: true,
        ..Self::NONE
    };
    pub const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };
    pub const LOGO: Self = Self {
        logo: true,
        ..Self::NONE
    };

    pub fn is_empty(self) -> bool {
        self == Self::NONE
    }

    /// These modifiers with `other` held (`pressed`) or released.
    pub fn with(self, other: Self, pressed: bool) -> Self {
        let pick = |held: bool, change: bool| if change { pressed } else { held };
        Self {
            shift: pick(self.shift, other.shift),
            ctrl: pick(self.ctrl, other.ctrl),
            alt: pick(self.alt, other.alt),
            logo: pick(self.logo, other.logo),
        }
    }

    fn egui(self) -> egui::Modifiers {
        egui::Modifiers {
            alt: self.alt,
            ctrl: self.ctrl,
            shift: self.shift,
            mac_cmd: false,
            command: self.ctrl,
        }
    }
}

/// A scroll amount: wheel ticks or continuous pixels, Wayland's sign
/// (positive is down and right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollDelta {
    Lines { x: f32, y: f32 },
    Pixels { x: f32, y: f32 },
}

/// The backend's axis source, independent of the UI toolkit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollSource {
    Unknown,
    Wheel,
    Finger,
}

/// One key press or release.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyInput {
    /// The physical key (evdev code + 8, as xkb and smithay count): what
    /// matches a release to its press while modifiers change the keysym.
    pub code: u32,
    /// The raw xkb keysym.
    pub keysym: u32,
    /// The composed text of a press, if it types anything.
    pub text: Option<String>,
    pub pressed: bool,
    pub repeat: bool,
}

/// Input for one UI instance. Positions are surface-local, in points
/// (physical pixels divided by the instance's scale).
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    PointerMoved {
        x: f32,
        y: f32,
    },
    PointerLeft,
    /// A Linux input-event code (`BTN_LEFT` is 0x110).
    Button {
        code: u32,
        pressed: bool,
    },
    Scroll {
        delta: ScrollDelta,
        source: ScrollSource,
    },
    Key(KeyInput),
    Modifiers(Modifiers),
    Focused(bool),
}

// xkb keysyms (stable: X11 protocol, keysymdef.h).
const XK_SHIFT_L: u32 = 0xffe1;
const XK_SHIFT_R: u32 = 0xffe2;
const XK_CONTROL_L: u32 = 0xffe3;
const XK_CONTROL_R: u32 = 0xffe4;
const XK_ALT_L: u32 = 0xffe9;
const XK_ALT_R: u32 = 0xffea;
const XK_SUPER_L: u32 = 0xffeb;
const XK_SUPER_R: u32 = 0xffec;

/// The modifier a keysym is, if it is one.
pub fn keysym_modifier(keysym: u32) -> Option<Modifiers> {
    match keysym {
        XK_SHIFT_L | XK_SHIFT_R => Some(Modifiers::SHIFT),
        XK_CONTROL_L | XK_CONTROL_R => Some(Modifiers::CTRL),
        XK_ALT_L | XK_ALT_R => Some(Modifiers::ALT),
        XK_SUPER_L | XK_SUPER_R => Some(Modifiers::LOGO),
        _ => None,
    }
}

/// A key event from smithay's keyboard handler.
pub fn key(code: u32, keysym: u32, utf8: Option<&str>, pressed: bool, repeat: bool) -> Input {
    let text = utf8
        .filter(|text| pressed && !text.is_empty())
        .map(str::to_owned);
    Input::Key(KeyInput {
        code,
        keysym,
        text,
        pressed,
        repeat,
    })
}

/// A scroll event: `discrete_*` are wheel ticks, `pixel_*` continuous
/// deltas (touchpad). Ticks win when present; `None` when both are zero.
pub fn wheel(
    discrete_x: i32,
    discrete_y: i32,
    pixel_x: f64,
    pixel_y: f64,
    source: ScrollSource,
) -> Option<Input> {
    let delta = if discrete_x != 0 || discrete_y != 0 {
        ScrollDelta::Lines {
            x: discrete_x as f32,
            y: discrete_y as f32,
        }
    } else if pixel_x != 0.0 || pixel_y != 0.0 {
        ScrollDelta::Pixels {
            x: pixel_x as f32,
            y: pixel_y as f32,
        }
    } else {
        return None;
    };
    Some(Input::Scroll { delta, source })
}

/// xkb keysym → egui key. Named keys by table; printable keys by their text.
fn egui_key(keysym: u32, text: Option<&str>) -> Option<egui::Key> {
    use egui::Key;
    const XK_F1: u32 = 0xffbe;
    const XK_F12: u32 = 0xffc9;
    let named = match keysym {
        0xff0d | 0xff8d => Key::Enter,
        0xff1b => Key::Escape,
        0xff08 => Key::Backspace,
        0xff09 | 0xfe20 => Key::Tab,
        0xffff => Key::Delete,
        0xff63 => Key::Insert,
        0xff50 => Key::Home,
        0xff57 => Key::End,
        0xff55 => Key::PageUp,
        0xff56 => Key::PageDown,
        0xff52 => Key::ArrowUp,
        0xff54 => Key::ArrowDown,
        0xff51 => Key::ArrowLeft,
        0xff53 => Key::ArrowRight,
        0x0020 => Key::Space,
        n if (XK_F1..=XK_F12).contains(&n) => {
            const F: [Key; 12] = [
                Key::F1,
                Key::F2,
                Key::F3,
                Key::F4,
                Key::F5,
                Key::F6,
                Key::F7,
                Key::F8,
                Key::F9,
                Key::F10,
                Key::F11,
                Key::F12,
            ];
            F[(n - XK_F1) as usize]
        }
        // Latin letters by keysym, so Ctrl+A is A whatever it types.
        n @ 0x0061..=0x007a => return Key::from_name(&char::from(n as u8 - 32).to_string()),
        // Digits and punctuation by keysym too: a release carries no text, so
        // a key resolved from text alone would never be released.
        n @ 0x0021..=0x007e => return Key::from_name(&char::from(n as u8).to_string()),
        _ => return text.and_then(Key::from_name),
    };
    Some(named)
}

fn egui_button(code: u32) -> Option<egui::PointerButton> {
    match code {
        0x110 => Some(egui::PointerButton::Primary),
        0x111 => Some(egui::PointerButton::Secondary),
        0x112 => Some(egui::PointerButton::Middle),
        0x113 => Some(egui::PointerButton::Extra1),
        0x114 => Some(egui::PointerButton::Extra2),
        _ => None,
    }
}

/// One runtime's input state: where the pointer is, which modifiers,
/// buttons and keys are held.
#[derive(Debug, Default)]
pub(crate) struct Translator {
    pointer: Option<egui::Pos2>,
    /// Where the pointer last was, kept after it leaves: a button released
    /// off the surface still releases there.
    last: Option<egui::Pos2>,
    buttons: Vec<egui::PointerButton>,
    /// The key each held physical key pressed, so its release is the same
    /// key whatever keysym, text or modifiers it carries by then.
    keys: Vec<(u32, egui::Key)>,
    modifiers: Modifiers,
}

impl Translator {
    /// Append the egui events for `input`.
    pub fn translate(&mut self, input: Input, out: &mut Vec<egui::Event>) {
        let modifiers = self.modifiers.egui();
        match input {
            Input::PointerMoved { x, y } => {
                let pos = egui::pos2(x, y);
                self.pointer = Some(pos);
                self.last = Some(pos);
                out.push(egui::Event::PointerMoved(pos));
            }
            Input::PointerLeft => {
                self.pointer = None;
                out.push(egui::Event::PointerGone);
            }
            Input::Button { code, pressed } => {
                let Some(button) = egui_button(code) else {
                    return;
                };
                if pressed {
                    // A press with no pointer over the surface has no position.
                    let Some(pos) = self.pointer else { return };
                    if !self.buttons.contains(&button) {
                        self.buttons.push(button);
                    }
                    out.push(egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers,
                    });
                } else if let Some(at) = self.buttons.iter().position(|held| *held == button) {
                    // Every press gets its release, wherever the pointer is.
                    self.buttons.remove(at);
                    let pos = self.pointer.or(self.last).unwrap_or(egui::Pos2::ZERO);
                    out.push(egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers,
                    });
                }
            }
            Input::Scroll { delta, source } => {
                // egui's wheel is positive up and left: the opposite of Wayland's.
                let (unit, delta) = match delta {
                    ScrollDelta::Lines { x, y } => (egui::MouseWheelUnit::Line, egui::vec2(-x, -y)),
                    ScrollDelta::Pixels { x, y } => {
                        (egui::MouseWheelUnit::Point, egui::vec2(-x, -y))
                    }
                };
                out.push(egui::Event::MouseWheel {
                    source: match source {
                        ScrollSource::Unknown => egui::MouseWheelSource::Unknown,
                        ScrollSource::Wheel => egui::MouseWheelSource::Wheel,
                        ScrollSource::Finger => egui::MouseWheelSource::Trackpad,
                    },
                    unit,
                    delta,
                    phase: egui::TouchPhase::Move,
                    modifiers,
                });
            }
            Input::Key(key) => {
                if let Some(changed) = keysym_modifier(key.keysym) {
                    self.modifiers = self.modifiers.with(changed, key.pressed);
                    out.push(egui::Event::ModifiersChanged(self.modifiers.egui()));
                    return;
                }
                let logical = if key.pressed {
                    // A key already held presses again as the key it first
                    // pressed, so its one release matches.
                    match self.keys.iter().find(|(code, _)| *code == key.code) {
                        Some((_, held)) => Some(*held),
                        None => {
                            let logical = egui_key(key.keysym, key.text.as_deref());
                            if let Some(logical) = logical {
                                self.keys.push((key.code, logical));
                            }
                            logical
                        }
                    }
                } else {
                    let at = self.keys.iter().position(|(code, _)| *code == key.code);
                    at.map(|at| self.keys.remove(at).1)
                };
                if let Some(logical) = logical {
                    out.push(egui::Event::Key {
                        key: logical,
                        physical_key: None,
                        pressed: key.pressed,
                        repeat: key.repeat,
                        modifiers,
                    });
                }
                // Typed text, unless a command chord is held.
                let chord = self.modifiers.ctrl || self.modifiers.alt || self.modifiers.logo;
                if let Some(text) = key
                    .text
                    .filter(|text| key.pressed && !chord && !text.chars().all(char::is_control))
                {
                    out.push(egui::Event::Text(text));
                }
            }
            Input::Modifiers(held) => {
                if held != self.modifiers {
                    self.modifiers = held;
                    out.push(egui::Event::ModifiersChanged(held.egui()));
                }
            }
            Input::Focused(focused) => {
                // Keys held when focus goes release here; their real releases
                // go elsewhere.
                if !focused {
                    for (_, logical) in self.keys.drain(..) {
                        out.push(egui::Event::Key {
                            key: logical,
                            physical_key: None,
                            pressed: false,
                            repeat: false,
                            modifiers,
                        });
                    }
                }
                if !focused && !self.modifiers.is_empty() {
                    self.modifiers = Modifiers::NONE;
                    out.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
                }
                out.push(egui::Event::WindowFocused(focused));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(inputs: Vec<Input>) -> Vec<egui::Event> {
        let mut translator = Translator::default();
        let mut out = Vec::new();
        for input in inputs {
            translator.translate(input, &mut out);
        }
        out
    }

    #[test]
    fn a_click_lands_where_the_pointer_is_with_the_held_modifiers() {
        let events = run(vec![
            key(50, XK_SHIFT_L, None, true, false),
            Input::PointerMoved { x: 10.0, y: 20.0 },
            Input::Button {
                code: 0x110,
                pressed: true,
            },
        ]);
        let Some(egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers,
        }) = events.last()
        else {
            panic!("{events:?}");
        };
        assert_eq!(*pos, egui::pos2(10.0, 20.0));
        assert_eq!(*button, egui::PointerButton::Primary);
        assert!(*pressed);
        assert!(modifiers.shift);
    }

    #[test]
    fn a_button_with_no_pointer_is_dropped() {
        let events = run(vec![
            Input::PointerLeft,
            Input::Button {
                code: 0x110,
                pressed: true,
            },
        ]);
        assert_eq!(events, vec![egui::Event::PointerGone]);
    }

    #[test]
    fn typing_yields_a_key_and_its_text_but_a_chord_only_the_key() {
        let events = run(vec![key(38, 0x0061, Some("a"), true, false)]);
        assert!(matches!(
            events[0],
            egui::Event::Key {
                key: egui::Key::A,
                pressed: true,
                ..
            }
        ));
        assert_eq!(events[1], egui::Event::Text("a".into()));
        let events = run(vec![
            key(37, XK_CONTROL_L, None, true, false),
            key(38, 0x0061, Some("a"), true, false),
        ]);
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, egui::Event::Text(_)))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            egui::Event::Key { key: egui::Key::A, modifiers, .. } if modifiers.ctrl
        )));
    }

    #[test]
    fn named_keys_and_releases() {
        let events = run(vec![
            key(9, 0xff1b, None, true, false),
            key(9, 0xff1b, None, false, false),
        ]);
        assert!(matches!(
            events[0],
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                ..
            }
        ));
        assert!(matches!(
            events[1],
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: false,
                ..
            }
        ));
        assert_eq!(events.len(), 2, "Escape types nothing");
        // A release with no press behind it is not invented.
        assert!(run(vec![key(9, 0xff1b, None, false, false)]).is_empty());
    }

    #[test]
    fn wheel_ticks_win_and_flip_to_egui_sign() {
        let Some(Input::Scroll { delta, source }) = wheel(0, 2, 0.0, 30.0, ScrollSource::Wheel)
        else {
            panic!()
        };
        assert_eq!(delta, ScrollDelta::Lines { x: 0.0, y: 2.0 });
        let events = run(vec![Input::Scroll { delta, source }]);
        assert!(matches!(
            events[0],
            egui::Event::MouseWheel { source: egui::MouseWheelSource::Wheel, unit: egui::MouseWheelUnit::Line, delta, .. } if delta == egui::vec2(0.0, -2.0)
        ));
        assert_eq!(wheel(0, 0, 0.0, 0.0, ScrollSource::Finger), None);
        for (source, expected) in [
            (ScrollSource::Finger, egui::MouseWheelSource::Trackpad),
            (ScrollSource::Unknown, egui::MouseWheelSource::Unknown),
        ] {
            let input = wheel(0, 0, 0.0, 3.0, source).unwrap();
            assert!(matches!(run(vec![input])[0], egui::Event::MouseWheel {
                source, unit: egui::MouseWheelUnit::Point, delta, ..
            } if source == expected && delta == egui::vec2(0.0, -3.0)));
        }
    }

    #[test]
    fn a_release_after_the_pointer_left_still_releases() {
        let events = run(vec![
            Input::PointerMoved { x: 5.0, y: 5.0 },
            Input::Button {
                code: 0x110,
                pressed: true,
            },
            Input::PointerLeft,
            Input::Button {
                code: 0x110,
                pressed: false,
            },
        ]);
        assert!(
            matches!(
                events.last(),
                Some(egui::Event::PointerButton { pressed: false, button: egui::PointerButton::Primary, pos, .. })
                    if *pos == egui::pos2(5.0, 5.0)
            ),
            "{events:?}"
        );
        // A release with no press behind it is not invented.
        let events = run(vec![
            Input::PointerMoved { x: 1.0, y: 1.0 },
            Input::Button {
                code: 0x110,
                pressed: false,
            },
        ]);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn punctuation_presses_and_releases_the_same_key() {
        for (keysym, text, expected) in [
            (0x002d, "-", egui::Key::Minus),
            (0x002f, "/", egui::Key::Slash),
        ] {
            let events = run(vec![
                key(20, keysym, Some(text), true, false),
                key(20, keysym, Some(text), false, false),
            ]);
            let keys: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Key { key, pressed, .. } => Some((*key, *pressed)),
                    _ => None,
                })
                .collect();
            assert_eq!(keys, vec![(expected, true), (expected, false)], "{text}");
        }
    }

    #[test]
    fn a_held_key_releases_as_pressed_whatever_the_modifiers_became() {
        let events = run(vec![
            key(38, 0x0061, Some("a"), true, false),
            key(50, XK_SHIFT_L, None, true, false),
            key(38, 0x0041, None, false, false),
        ]);
        let releases: Vec<_> = events
            .iter()
            .filter(|event| matches!(event, egui::Event::Key { pressed: false, .. }))
            .collect();
        assert_eq!(
            releases.len(),
            1,
            "the physical key releases once: {events:?}"
        );
        assert!(
            matches!(
                releases[0],
                egui::Event::Key {
                    key: egui::Key::A,
                    ..
                }
            ),
            "as the key it pressed: {events:?}"
        );
    }

    #[test]
    fn a_held_key_pressed_again_after_shift_stays_one_key() {
        let events = run(vec![
            key(21, 0x003d, Some("="), true, false),
            key(50, XK_SHIFT_L, None, true, false),
            key(21, 0x002b, Some("+"), true, true),
            key(21, 0x002b, None, false, false),
        ]);
        let keys: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key { key, pressed, .. } => Some((*key, *pressed)),
                _ => None,
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                (egui::Key::Equals, true),
                (egui::Key::Equals, true),
                (egui::Key::Equals, false)
            ],
            "{events:?}"
        );
    }

    #[test]
    fn losing_focus_releases_held_modifiers() {
        let events = run(vec![
            key(37, XK_CONTROL_L, None, true, false),
            Input::Focused(false),
        ]);
        assert_eq!(
            events[1],
            egui::Event::ModifiersChanged(egui::Modifiers::NONE)
        );
        assert_eq!(events[2], egui::Event::WindowFocused(false));
        let events = run(vec![
            key(38, 0x0061, Some("a"), true, false),
            Input::Focused(false),
        ]);
        assert!(
            events.iter().any(|event| matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::A,
                    pressed: false,
                    ..
                }
            )),
            "a key held when focus goes is released: {events:?}"
        );
    }
}
