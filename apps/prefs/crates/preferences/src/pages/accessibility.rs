// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! System › Accessibility (spec 1 §4.13; the matrix §14): Vision, Hearing
//! and Mobility, the Magnifier and On-screen keyboard pages under it, and
//! the MixOS-only Motion section (reduced motion and text size). A missing
//! backend names itself once: on a page it waits on, or on the one row that
//! alone is off.
use super::{ChoiceStyle, Control, Lit, Opt, Page, opt, row, section, sections, untitled};

const FILTERS: &[Opt] = &[
    opt("none", "opt-filter-off"),
    opt("greyscale", "opt-filter-greyscale"),
    opt("deuteranopia", "opt-filter-deuteranopia"),
    opt("protanopia", "opt-filter-protanopia"),
    opt("tritanopia", "opt-filter-tritanopia"),
];
const ZOOM: &[Opt] = &[
    Opt {
        value: Lit::I(25),
        label: "opt-zoom-25",
    },
    Opt {
        value: Lit::I(50),
        label: "opt-zoom-50",
    },
    Opt {
        value: Lit::I(100),
        label: "opt-zoom-100",
    },
    Opt {
        value: Lit::I(150),
        label: "opt-zoom-150",
    },
    Opt {
        value: Lit::I(200),
        label: "opt-zoom-200",
    },
];
const MOVEMENT: &[Opt] = &[
    opt("continuous", "opt-movement-continuous"),
    opt("on_edge", "opt-movement-on-edge"),
    opt("centred", "opt-movement-centred"),
];

pub(super) fn pages() -> Vec<Page> {
    let mut accessibility = super::page(
        "accessibility",
        "accessibility",
        None,
        sections(vec![
            section(
                "vision",
                vec![
                    row(
                        "screen_reader",
                        Control::Toggle {
                            key: "accessibility.screen_reader",
                        },
                    )
                    .backend("backend-screen-reader"),
                    row("magnifier", Control::Page { page: "magnifier" })
                        .backend("backend-compd-magnifier"),
                    row(
                        "contrast",
                        Control::Flag {
                            key: "appearance.contrast",
                            on: Lit::S("high"),
                            off: Lit::S("normal"),
                        },
                    ),
                    row(
                        "invert",
                        Control::Toggle {
                            key: "accessibility.invert",
                        },
                    )
                    .backend("backend-compd-invert"),
                    row(
                        "color_filter",
                        Control::Choice {
                            key: "accessibility.color_filter",
                            style: ChoiceStyle::Dropdown,
                            options: FILTERS,
                        },
                    )
                    .backend("backend-compd-filters"),
                    row(
                        "cursor_shake",
                        Control::Toggle {
                            key: "accessibility.cursor_shake",
                        },
                    )
                    .backend("backend-compd-pointer-shake"),
                ],
            ),
            section(
                "hearing",
                vec![
                    row(
                        "mono_sound",
                        Control::Toggle {
                            key: "accessibility.mono_sound",
                        },
                    )
                    .backend("backend-audio"),
                ],
            ),
            section(
                "mobility",
                vec![row("osk", Control::Page { page: "osk" })],
            ),
            section(
                "motion",
                vec![
                    row(
                        "reduced_motion",
                        Control::Toggle {
                            key: "accessibility.reduced_motion",
                        },
                    ),
                    row(
                        "text_scale",
                        Control::Slider {
                            key: "accessibility.text_scale",
                            min: 0.5,
                            max: 3.0,
                            step: 0.05,
                            scale: 100.0,
                            show_value: true,
                            start: None,
                            end: None,
                        },
                    ),
                ],
            ),
        ]),
    );

    let mut magnifier = super::page(
        "magnifier",
        "zoom-in",
        Some("accessibility"),
        sections(vec![
            section(
                "magnifier",
                vec![
                    row(
                        "enabled",
                        Control::Toggle {
                            key: "accessibility.magnifier.enabled",
                        },
                    ),
                    row(
                        "mouse_zoom",
                        Control::Toggle {
                            key: "accessibility.magnifier.mouse_zoom",
                        },
                    ),
                    row(
                        "overlay",
                        Control::Toggle {
                            key: "accessibility.magnifier.overlay",
                        },
                    ),
                    row(
                        "increment",
                        Control::Choice {
                            key: "accessibility.magnifier.increment",
                            style: ChoiceStyle::Dropdown,
                            options: ZOOM,
                        },
                    ),
                    row(
                        "on_login",
                        Control::Toggle {
                            key: "accessibility.magnifier.on_login",
                        },
                    ),
                ],
            ),
            section(
                "movement",
                vec![row(
                    "movement",
                    Control::Choice {
                        key: "accessibility.magnifier.movement",
                        style: ChoiceStyle::Radio,
                        options: MOVEMENT,
                    },
                )],
            ),
        ]),
    );
    magnifier.backend = Some("backend-compd-magnifier");

    let mut osk = super::page(
        "osk",
        "keyboard",
        Some("accessibility"),
        sections(vec![untitled(
            "keyboard",
            vec![
                row(
                    "always_show",
                    Control::Toggle {
                        key: "accessibility.osk.always_show",
                    },
                ),
                row(
                    "gamepad_shortcut",
                    Control::Toggle {
                        key: "accessibility.osk.gamepad_shortcut",
                    },
                ),
                row(
                    "show_on_text_input",
                    Control::Toggle {
                        key: "accessibility.osk.show_on_text_input",
                    },
                ),
            ],
        )]),
    );
    osk.backend = Some("backend-osk");

    vec![accessibility, magnifier, osk]
}
