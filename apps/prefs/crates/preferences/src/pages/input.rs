// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Input devices › Keyboard (spec 1 §4.9.4; the matrix §9.1): the input
//! sources in order, the special keys (alternate characters, compose,
//! Caps Lock) as drawers of radios, the shortcuts sub-page, repeat delay
//! and rate, and the numlock state on boot. inputd applies every key
//! through xkb; the layouts offered come from the system's xkb rules
//! ([`crate::xkb`]).
use super::{Body, ChoiceStyle, Control, Drawer, Opt, Page, opt, row, section, untitled};
use crate::xkb::{EXTENDED, LAYOUTS};

const ALT_CHARS: &[Opt] = &[
    opt("none", "opt-key-none"),
    opt("lalt", "opt-key-lalt"),
    opt("ralt", "opt-key-ralt"),
    opt("lwin", "opt-key-lwin"),
    opt("rwin", "opt-key-rwin"),
    opt("menu", "opt-key-menu"),
    opt("caps", "opt-key-caps"),
];
const COMPOSE: &[Opt] = &[
    opt("none", "opt-key-none"),
    opt("ralt", "opt-key-ralt"),
    opt("lwin", "opt-key-lwin"),
    opt("rwin", "opt-key-rwin"),
    opt("menu", "opt-key-menu"),
    opt("rctrl", "opt-key-rctrl"),
    opt("caps", "opt-key-caps"),
    opt("sclk", "opt-key-sclk"),
    opt("prsc", "opt-key-prsc"),
];
const CAPS_LOCK: &[Opt] = &[
    opt("caps", "opt-caps-caps"),
    opt("escape", "opt-caps-escape"),
    opt("swapescape", "opt-caps-swapescape"),
    opt("backspace", "opt-caps-backspace"),
    opt("super", "opt-caps-super"),
    opt("ctrl", "opt-caps-ctrl"),
    opt("swapctrl", "opt-caps-swapctrl"),
];
const NUMLOCK: &[Opt] = &[
    opt("off", "opt-numlock-off"),
    opt("on", "opt-numlock-on"),
    opt("last", "opt-numlock-last"),
];

fn radios(id: &'static str, key: &'static str, options: &'static [Opt]) -> super::Row {
    row(
        id,
        Control::Choice {
            key,
            style: ChoiceStyle::Radio,
            options,
        },
    )
}

pub(super) fn keyboard() -> Page {
    let sections = vec![
        section(
            "sources",
            vec![row(
                "layouts",
                Control::Ordered {
                    key: "input.keyboard.layouts",
                    sources: &[LAYOUTS, EXTENDED],
                    add: Some("add_source"),
                },
            )],
        ),
        section(
            "special",
            vec![
                row("alt_chars", Control::Drawer { drawer: "alt_chars" }),
                row("compose", Control::Drawer { drawer: "compose" }),
                row("caps", Control::Drawer { drawer: "caps" }),
            ],
        ),
        section(
            "shortcuts",
            vec![row(
                "shortcuts",
                Control::Page {
                    page: "keyboard_shortcuts",
                },
            )],
        ),
        section(
            "typing",
            vec![
                row(
                    "repeat_delay",
                    Control::Slider {
                        key: "input.keyboard.repeat_delay_ms",
                        min: 200.0,
                        max: 1000.0,
                        step: 50.0,
                        scale: 1.0,
                        show_value: false,
                        start: Some("opt-delay-short"),
                        end: Some("opt-delay-long"),
                    },
                ),
                row(
                    "repeat_rate",
                    Control::Slider {
                        key: "input.keyboard.repeat_rate",
                        min: 5.0,
                        max: 45.0,
                        step: 5.0,
                        scale: 1.0,
                        show_value: false,
                        start: Some("opt-rate-slow"),
                        end: Some("opt-rate-fast"),
                    },
                ),
            ],
        ),
        section(
            "numlock",
            vec![row("numlock", Control::Drawer { drawer: "numlock" })],
        ),
    ];
    let drawers = vec![
        Drawer {
            id: "add_source",
            sections: vec![untitled(
                "add",
                vec![row(
                    "add_layout",
                    Control::Append {
                        key: "input.keyboard.layouts",
                        source: LAYOUTS,
                        more: Some(EXTENDED),
                    },
                )],
            )],
        },
        Drawer {
            id: "alt_chars",
            sections: vec![untitled(
                "alt_chars",
                vec![radios(
                    "alt_chars_key",
                    "input.keyboard.alt_chars_key",
                    ALT_CHARS,
                )],
            )],
        },
        Drawer {
            id: "compose",
            sections: vec![
                untitled("compose_about", vec![row("compose_about", Control::Info)]),
                untitled(
                    "compose",
                    vec![radios(
                        "compose_key",
                        "input.keyboard.compose_key",
                        COMPOSE,
                    )],
                ),
            ],
        },
        Drawer {
            id: "caps",
            sections: vec![untitled(
                "caps",
                vec![radios("caps_lock", "input.keyboard.caps_lock", CAPS_LOCK)],
            )],
        },
        Drawer {
            id: "numlock",
            sections: vec![untitled(
                "numlock",
                vec![radios(
                    "numlock_boot",
                    "input.keyboard.numlock_boot",
                    NUMLOCK,
                )],
            )],
        },
    ];
    super::page(
        "keyboard",
        "keyboard",
        Some("input"),
        Body::Sections { sections, drawers },
    )
}
