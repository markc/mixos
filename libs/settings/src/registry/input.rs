// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.9 `input.*`: the keyboard is applied by inputd (xkb), pointers and
//! tablets by compd.
use super::{DefaultValue as D, Domain::*, KeyDef as K, Kind, Kind::*};

const fn keyboard(path: &'static str, kind: Kind, default: D) -> K {
    K::new(path, kind, default)
        .on("input/keyboard", &[Input])
        .applied("inputd")
}
const fn pointer(page: &'static str, path: &'static str, kind: Kind, default: D) -> K {
    K::new(path, kind, default)
        .on(page, &[Input])
        .applied("compd")
}
const fn mouse(path: &'static str, kind: Kind, default: D) -> K {
    pointer("input/mouse", path, kind, default)
}
const fn touchpad(path: &'static str, kind: Kind, default: D) -> K {
    pointer("input/touchpad", path, kind, default)
}
const BUTTON: Kind = Enum(&["left", "right"]);
const SPEED: Kind = Int {
    min: 0,
    max: 100,
    step: 1,
};
const SCROLL: Kind = Int {
    min: 1,
    max: 100,
    step: 1,
};

pub(super) const KEYS: &[K] = &[
    keyboard(
        "input.keyboard.layouts",
        List(&XkbLayout, 1, 8),
        D::Json(r#"[["us",""]]"#),
    ),
    keyboard(
        "input.keyboard.alt_chars_key",
        Enum(&["none", "lalt", "ralt", "lwin", "rwin", "menu", "caps"]),
        D::Text("none"),
    ),
    keyboard(
        "input.keyboard.compose_key",
        Enum(&[
            "none", "ralt", "lwin", "rwin", "menu", "rctrl", "caps", "sclk", "prsc",
        ]),
        D::Text("none"),
    ),
    keyboard(
        "input.keyboard.caps_lock",
        Enum(&[
            "caps",
            "escape",
            "swapescape",
            "backspace",
            "super",
            "ctrl",
            "swapctrl",
        ]),
        D::Text("caps"),
    ),
    keyboard(
        "input.keyboard.repeat_delay_ms",
        Int {
            min: 200,
            max: 1000,
            step: 50,
        },
        D::Int(600),
    ),
    keyboard(
        "input.keyboard.repeat_rate",
        Int {
            min: 5,
            max: 45,
            step: 5,
        },
        D::Int(25),
    ),
    keyboard(
        "input.keyboard.numlock_boot",
        Enum(&["off", "on", "last"]),
        D::Text("off"),
    ),
    mouse("input.mouse.primary_button", BUTTON, D::Text("left")),
    mouse("input.mouse.speed", SPEED, D::Int(57)),
    mouse("input.mouse.acceleration", Bool, D::Bool(true)),
    mouse("input.mouse.scroll_speed", SCROLL, D::Int(50)),
    mouse("input.mouse.natural_scroll", Bool, D::Bool(false)),
    touchpad("input.touchpad.primary_button", BUTTON, D::Text("left")),
    touchpad("input.touchpad.speed", SPEED, D::Int(57)),
    touchpad("input.touchpad.acceleration", Bool, D::Bool(true)),
    touchpad("input.touchpad.disable_while_typing", Bool, D::Bool(true)),
    touchpad(
        "input.touchpad.click_method",
        Enum(&["fingers", "areas"]),
        D::Text("fingers"),
    ),
    touchpad("input.touchpad.tap_to_click", Bool, D::Bool(true)),
    touchpad(
        "input.touchpad.scroll_method",
        Enum(&["two_finger", "edge"]),
        D::Text("two_finger"),
    ),
    touchpad("input.touchpad.scroll_speed", SCROLL, D::Int(50)),
    touchpad("input.touchpad.natural_scroll", Bool, D::Bool(true)),
    pointer(
        "input/tablet",
        "input.tablet.<tablet>.enabled",
        Bool,
        D::Bool(true),
    ),
    pointer(
        "input/tablet",
        "input.tablet.<tablet>.output",
        ConnectorOr(&[]),
        D::Null,
    ),
];
