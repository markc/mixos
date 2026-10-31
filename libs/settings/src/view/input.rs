// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `input.*`: the keyboard (inputd, xkb), pointers and tablets (compd).
use super::{Reader, View, family};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub keyboard: Keyboard,
    pub mouse: Mouse,
    pub touchpad: Touchpad,
    /// Tablets with an authored value, by name; others take the defaults.
    pub tablets: BTreeMap<String, Tablet>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Keyboard {
    /// xkb `(layout, variant)` pairs, 1 to 8, in order.
    pub layouts: Vec<(String, String)>,
    pub alt_chars_key: String,
    pub compose_key: String,
    pub caps_lock: String,
    pub repeat_delay_ms: u32,
    /// Repeats per second.
    pub repeat_rate: u32,
    /// `off`, `on` or `last`.
    pub numlock_boot: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mouse {
    /// `left` or `right`.
    pub primary_button: String,
    /// 0 to 100; libinput's speed is `v / 70.71 - 0.81`.
    pub speed: u32,
    pub acceleration: bool,
    /// 1 to 100; the factor is `2^((v - 50) / 10)`.
    pub scroll_speed: u32,
    pub natural_scroll: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Touchpad {
    pub primary_button: String,
    pub speed: u32,
    pub acceleration: bool,
    pub disable_while_typing: bool,
    /// `fingers` or `areas`.
    pub click_method: String,
    pub tap_to_click: bool,
    /// `two_finger` or `edge`.
    pub scroll_method: String,
    pub scroll_speed: u32,
    pub natural_scroll: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tablet {
    pub enabled: bool,
    /// The output it maps to; None is every output.
    pub output: Option<String>,
}

fn layouts(r: &Reader) -> Vec<(String, String)> {
    r.value("input.keyboard.layouts")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pair| {
            let pair = super::strings(pair);
            match pair.as_slice() {
                [layout, variant] => Some((layout.clone(), variant.clone())),
                _ => None,
            }
        })
        .collect()
}

impl View for Input {
    fn read(r: &Reader) -> Self {
        Self {
            keyboard: Keyboard {
                layouts: layouts(r),
                alt_chars_key: r.text("input.keyboard.alt_chars_key"),
                compose_key: r.text("input.keyboard.compose_key"),
                caps_lock: r.text("input.keyboard.caps_lock"),
                repeat_delay_ms: r.uint("input.keyboard.repeat_delay_ms"),
                repeat_rate: r.uint("input.keyboard.repeat_rate"),
                numlock_boot: r.text("input.keyboard.numlock_boot"),
            },
            mouse: Mouse {
                primary_button: r.text("input.mouse.primary_button"),
                speed: r.uint("input.mouse.speed"),
                acceleration: r.bool("input.mouse.acceleration"),
                scroll_speed: r.uint("input.mouse.scroll_speed"),
                natural_scroll: r.bool("input.mouse.natural_scroll"),
            },
            touchpad: Touchpad {
                primary_button: r.text("input.touchpad.primary_button"),
                speed: r.uint("input.touchpad.speed"),
                acceleration: r.bool("input.touchpad.acceleration"),
                disable_while_typing: r.bool("input.touchpad.disable_while_typing"),
                click_method: r.text("input.touchpad.click_method"),
                tap_to_click: r.bool("input.touchpad.tap_to_click"),
                scroll_method: r.text("input.touchpad.scroll_method"),
                scroll_speed: r.uint("input.touchpad.scroll_speed"),
                natural_scroll: r.bool("input.touchpad.natural_scroll"),
            },
            tablets: family(r, "input.tablet.", 2, |t| Tablet {
                enabled: r.bool(&format!("input.tablet.{t}.enabled")),
                output: r.opt_text(&format!("input.tablet.{t}.output")),
            }),
        }
    }
}
