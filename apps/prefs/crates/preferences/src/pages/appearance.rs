// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Desktop › Appearance (spec 1 §4.2; the matrix §2): mode and colours,
//! style, density, the window hint and gaps, fonts and the toolkit
//! exports. One value per key; settingsd and the exporters keep light and
//! dark consistent, so the page writes each key once.
use super::{
    Body, ChoiceStyle, Cond, Control, Drawer, Extra, Lit, Page, PickList, opt, row, section,
    untitled,
};

const PINNED: Cond = Cond::Unset("appearance.source");
const ROUNDNESS: &[super::Opt] = &[
    opt("round", "opt-round"),
    opt("slightly_round", "opt-slightly-round"),
    opt("square", "opt-square"),
];
const DENSITY: &[super::Opt] = &[
    opt("compact", "opt-compact"),
    opt("comfortable", "opt-comfortable"),
    opt("spacious", "opt-spacious"),
];

pub(super) fn page() -> Page {
    let mode_and_colours = section(
        "colors",
        vec![
            row("mode", Control::Mode).enable(&[(PINNED, "row-appearance-mode-pinned")]),
            // Auto needs a sunrise and sunset: daylight leaves its state
            // null in a zone without one (UTC, Etc/GMT±N).
            row("auto", Control::AutoMode).enable(&[
                (PINNED, "row-appearance-mode-pinned"),
                (
                    Cond::Set("state.appearance.daylight"),
                    "row-appearance-auto-no-daylight",
                ),
            ]),
            row("scheme", Control::Scheme),
            row("accent", Control::Accent),
            row(
                "window_bg",
                Control::Colour {
                    key: "appearance.window_bg",
                },
            ),
            row(
                "container_bg",
                Control::Colour {
                    key: "appearance.container_bg",
                },
            ),
            row(
                "text_tint",
                Control::Colour {
                    key: "appearance.text_tint",
                },
            ),
            row(
                "control_tint",
                Control::Colour {
                    key: "appearance.control_tint",
                },
            ),
            row("hint_accent", Control::HintAccent),
            row(
                "hint_colour",
                Control::Colour {
                    key: "window.hint_colour",
                },
            )
            .show(Cond::Not("window.hint_colour", Lit::S("accent"))),
            row(
                "contrast",
                Control::Flag {
                    key: "appearance.contrast",
                    on: Lit::S("high"),
                    off: Lit::S("normal"),
                },
            ),
        ],
    );
    let style = section(
        "style",
        vec![
            row(
                "roundness",
                Control::Choice {
                    key: "appearance.roundness",
                    style: ChoiceStyle::Images,
                    options: ROUNDNESS,
                },
            ),
            row("frosted", Control::Drawer { drawer: "frosted" }),
        ],
    );
    let density = section(
        "density",
        vec![row(
            "density",
            Control::Choice {
                key: "appearance.density",
                style: ChoiceStyle::Radio,
                options: DENSITY,
            },
        )],
    );
    let windows = section(
        "windows",
        vec![
            row(
                "active_hint",
                Control::Spin {
                    key: "window.active_hint",
                    min: 0,
                    max: 64,
                    step: 1,
                },
            ),
            row(
                "gaps",
                Control::Spin {
                    key: "window.gaps",
                    min: 0,
                    max: 500,
                    step: 1,
                },
            )
            .backend("backend-compd-gaps"),
        ],
    );
    let experimental = section(
        "experimental",
        vec![
            row(
                "font",
                Control::Pick {
                    key: "appearance.font_family",
                    list: PickList::Fonts,
                },
            ),
            row(
                "mono",
                Control::Pick {
                    key: "appearance.mono_family",
                    list: PickList::MonoFonts,
                },
            ),
            row("toolkits", Control::Drawer { drawer: "toolkits" }),
            row("shadows", Control::Drawer { drawer: "shadows" }),
        ],
    );
    let reset = untitled(
        "reset",
        vec![row(
            "reset",
            Control::Reset {
                keys: &[
                    "appearance.scheme",
                    "appearance.accent",
                    "appearance.window_bg",
                    "appearance.container_bg",
                    "appearance.text_tint",
                    "appearance.control_tint",
                    "appearance.contrast",
                    "appearance.roundness",
                    "appearance.density",
                    "appearance.frosted.*",
                    "window.hint_colour",
                    "window.active_hint",
                    "window.gaps",
                ],
            },
        )],
    );
    let frosted = Drawer {
        id: "frosted",
        sections: vec![untitled(
            "frosted",
            vec![
                row(
                    "frosted_panel",
                    Control::Toggle {
                        key: "appearance.frosted.panel",
                    },
                )
                .backend("backend-quoin-frosted"),
                row(
                    "frosted_applets",
                    Control::Toggle {
                        key: "appearance.frosted.applets",
                    },
                )
                .backend("backend-quoin-frosted"),
                row(
                    "frosted_system",
                    Control::Toggle {
                        key: "appearance.frosted.system",
                    },
                )
                .backend("backend-compd-frosted"),
                row(
                    "frosted_windows",
                    Control::Toggle {
                        key: "appearance.frosted.windows",
                    },
                )
                .backend("backend-compd-frosted"),
                row(
                    "frosted_thickness",
                    Control::Slider {
                        key: "appearance.frosted.thickness",
                        min: 0.0,
                        max: 13.0,
                        step: 1.0,
                        scale: 1.0,
                        show_value: false,
                        start: Some("opt-less"),
                        end: Some("opt-more"),
                    },
                )
                .backend("backend-compd-blur"),
                row(
                    "frosted_opacity",
                    Control::Slider {
                        key: "appearance.frosted.opacity",
                        min: 0.0,
                        max: 1.0,
                        step: 0.05,
                        scale: 1.0,
                        show_value: false,
                        start: Some("opt-less"),
                        end: Some("opt-more"),
                    },
                )
                .backend("backend-compd-translucent"),
            ],
        )],
    };
    let toolkits = Drawer {
        id: "toolkits",
        sections: vec![
            untitled(
                "exports",
                vec![
                    row(
                        "export_toolkits",
                        Control::Toggle {
                            key: "appearance.export_toolkits",
                        },
                    ),
                    row(
                        "export_desktop",
                        Control::Toggle {
                            key: "appearance.export_desktop",
                        },
                    ),
                ],
            ),
            section(
                "icons",
                vec![row(
                    "icon_theme",
                    Control::Pick {
                        key: "appearance.icon_theme",
                        list: PickList::IconThemes,
                    },
                )],
            ),
        ],
    };
    let shadows = Drawer {
        id: "shadows",
        sections: vec![
            section(
                "floating",
                vec![row(
                    "clip_floating",
                    Control::Toggle {
                        key: "window.clip_floating",
                    },
                )],
            ),
            section(
                "tiled",
                vec![
                    row(
                        "clip_tiled",
                        Control::Toggle {
                            key: "window.clip_tiled",
                        },
                    ),
                    row(
                        "shadow_tiled",
                        Control::Toggle {
                            key: "window.shadow_tiled",
                        },
                    ),
                ],
            ),
        ],
    };
    let mut page = super::page(
        "appearance",
        "palette",
        Some("desktop"),
        Body::Sections {
            sections: vec![
                mode_and_colours,
                style,
                density,
                windows,
                experimental,
                reset,
            ],
            drawers: vec![frosted, toolkits, shadows],
        },
    );
    page.extras = vec![
        Extra {
            id: "import",
            command: None,
            backend: None,
        },
        Extra {
            id: "export",
            command: None,
            backend: None,
        },
    ];
    page
}
