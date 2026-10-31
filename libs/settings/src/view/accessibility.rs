// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `accessibility.*`, with `reduced_motion` and `text_scale` (moved from
//! `ui.*`, ruling 4).
use super::{Reader, View};

#[derive(Clone, Debug, PartialEq)]
pub struct Accessibility {
    pub screen_reader: bool,
    pub invert: bool,
    /// `none`, `greyscale`, `deuteranopia`, `protanopia` or `tritanopia`.
    pub color_filter: String,
    pub cursor_shake: bool,
    pub mono_sound: bool,
    pub reduced_motion: bool,
    /// 0.5 to 3.0.
    pub text_scale: f64,
    pub magnifier: Magnifier,
    pub osk: Osk,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Magnifier {
    pub enabled: bool,
    pub mouse_zoom: bool,
    pub overlay: bool,
    /// Percent, 25 to 200 in steps of 25.
    pub increment: u32,
    pub on_login: bool,
    /// `continuous`, `on_edge` or `centred`.
    pub movement: String,
}

/// The on-screen keyboard.
#[derive(Clone, Debug, PartialEq)]
pub struct Osk {
    pub always_show: bool,
    pub gamepad_shortcut: bool,
    pub show_on_text_input: bool,
}

impl View for Accessibility {
    fn read(r: &Reader) -> Self {
        Self {
            screen_reader: r.bool("accessibility.screen_reader"),
            invert: r.bool("accessibility.invert"),
            color_filter: r.text("accessibility.color_filter"),
            cursor_shake: r.bool("accessibility.cursor_shake"),
            mono_sound: r.bool("accessibility.mono_sound"),
            reduced_motion: r.bool("accessibility.reduced_motion"),
            text_scale: r.number("accessibility.text_scale"),
            magnifier: Magnifier {
                enabled: r.bool("accessibility.magnifier.enabled"),
                mouse_zoom: r.bool("accessibility.magnifier.mouse_zoom"),
                overlay: r.bool("accessibility.magnifier.overlay"),
                increment: r.uint("accessibility.magnifier.increment"),
                on_login: r.bool("accessibility.magnifier.on_login"),
                movement: r.text("accessibility.magnifier.movement"),
            },
            osk: Osk {
                always_show: r.bool("accessibility.osk.always_show"),
                gamepad_shortcut: r.bool("accessibility.osk.gamepad_shortcut"),
                show_on_text_input: r.bool("accessibility.osk.show_on_text_input"),
            },
        }
    }
}
