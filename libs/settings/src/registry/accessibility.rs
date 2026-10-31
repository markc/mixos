// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.14 `accessibility.*`. `reduced_motion` and `text_scale` moved here
//! from `ui.*`.
use super::{DefaultValue as D, Domain, Domain::*, KeyDef as K, Kind, Kind::*};

const PAGE: &str = "accessibility";

const fn k(path: &'static str, kind: Kind, default: D, domains: &'static [Domain]) -> K {
    K::new(path, kind, default).on(PAGE, domains)
}
const fn magnifier(path: &'static str, kind: Kind, default: D) -> K {
    K::new(path, kind, default).on("accessibility/magnifier", &[Input])
}
const fn osk(path: &'static str) -> K {
    K::new(path, Bool, D::Bool(false)).on("accessibility/keyboard", &[Input])
}

pub(super) const KEYS: &[K] = &[
    k("accessibility.screen_reader", Bool, D::Bool(false), &[System]).applied("screen-reader"),
    k("accessibility.invert", Bool, D::Bool(false), &[Paint]),
    k(
        "accessibility.color_filter",
        Enum(&[
            "none",
            "greyscale",
            "deuteranopia",
            "protanopia",
            "tritanopia",
        ]),
        D::Text("none"),
        &[Paint],
    ),
    k("accessibility.cursor_shake", Bool, D::Bool(false), &[Input]),
    k("accessibility.mono_sound", Bool, D::Bool(false), &[System]).applied("audio"),
    k(
        "accessibility.reduced_motion",
        Bool,
        D::Bool(false),
        &[Motion],
    ),
    k(
        "accessibility.text_scale",
        Number {
            min: 0.5,
            max: 3.0,
            step: 0.0,
        },
        D::Number(1.0),
        &[Text, Layout],
    ),
    magnifier("accessibility.magnifier.enabled", Bool, D::Bool(false)),
    magnifier("accessibility.magnifier.mouse_zoom", Bool, D::Bool(true)),
    magnifier("accessibility.magnifier.overlay", Bool, D::Bool(true)),
    magnifier(
        "accessibility.magnifier.increment",
        Int {
            min: 25,
            max: 200,
            step: 25,
        },
        D::Int(50),
    ),
    magnifier("accessibility.magnifier.on_login", Bool, D::Bool(false)),
    magnifier(
        "accessibility.magnifier.movement",
        Enum(&["continuous", "on_edge", "centred"]),
        D::Text("continuous"),
    ),
    osk("accessibility.osk.always_show"),
    osk("accessibility.osk.gamepad_shortcut"),
    osk("accessibility.osk.show_on_text_input"),
];
