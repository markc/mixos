// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Desktop › Panel and Desktop › Dock (spec 1 §4.3; the matrix §3): one
//! form over `shell.panels.top` (COSMIC's Panel) and `shell.panels.bottom`
//! (its Dock, and Quoin's panel). The panel's radius and padding derive
//! from roundness, gap and extend; they are never keys.
use super::{Body, ChoiceStyle, Control, Lit, Page, opt, row, section, untitled};

const EDGES: &[super::Opt] = &[
    opt("left", "opt-left"),
    opt("right", "opt-right"),
    opt("top", "opt-top"),
    opt("bottom", "opt-bottom"),
];
const LOOKS: &[super::Opt] = &[
    opt("match", "opt-match-desktop"),
    opt("light", "opt-light"),
    opt("dark", "opt-dark"),
];
const OUTPUTS: &[super::Opt] = &[opt("all", "opt-all-displays")];
const SIZES: &[Lit] = &[Lit::I(32), Lit::I(40), Lit::I(56), Lit::I(64), Lit::I(80)];

macro_rules! bar {
    ($page:literal, $panel:literal, $icon:literal, $applets:literal) => {{
        let key = |leaf: &'static str| -> &'static str {
            match leaf {
                "enabled" => concat!("shell.panels.", $panel, ".enabled"),
                "autohide" => concat!("shell.panels.", $panel, ".autohide"),
                "edge" => concat!("shell.panels.", $panel, ".edge"),
                "output" => concat!("shell.panels.", $panel, ".output"),
                "gap" => concat!("shell.panels.", $panel, ".gap"),
                "extend" => concat!("shell.panels.", $panel, ".extend"),
                "appearance" => concat!("shell.panels.", $panel, ".appearance"),
                "thickness" => concat!("shell.panels.", $panel, ".thickness"),
                "opacity" => concat!("shell.panels.", $panel, ".opacity"),
                _ => unreachable!("a panel key"),
            }
        };
        let sections = vec![
            untitled(
                "bar",
                vec![row("enabled", Control::Toggle { key: key("enabled") })],
            ),
            section(
                "behavior",
                vec![
                    row("autohide", Control::Toggle { key: key("autohide") }),
                    row(
                        "edge",
                        Control::Choice {
                            key: key("edge"),
                            style: ChoiceStyle::Dropdown,
                            options: EDGES,
                        },
                    ),
                    row(
                        "output",
                        Control::Choice {
                            key: key("output"),
                            style: ChoiceStyle::Dropdown,
                            options: OUTPUTS,
                        },
                    ),
                ],
            ),
            section(
                "style",
                vec![
                    row("gap", Control::Toggle { key: key("gap") }),
                    row("extend", Control::Toggle { key: key("extend") }),
                    row(
                        "appearance",
                        Control::Choice {
                            key: key("appearance"),
                            style: ChoiceStyle::Dropdown,
                            options: LOOKS,
                        },
                    ),
                    row(
                        "size",
                        Control::Steps {
                            key: key("thickness"),
                            steps: SIZES,
                            start: "opt-small",
                            end: "opt-large",
                        },
                    ),
                    row(
                        "opacity",
                        Control::Slider {
                            key: key("opacity"),
                            min: 0.0,
                            max: 1.0,
                            step: 0.01,
                            scale: 100.0,
                            show_value: true,
                            start: None,
                            end: None,
                        },
                    ),
                ],
            ),
            section(
                "configuration",
                vec![row("applets", Control::Page { page: $applets })],
            ),
            untitled(
                "reset",
                vec![row(
                    "reset",
                    Control::Reset {
                        keys: &[concat!("shell.panels.", $panel, ".*")],
                    },
                )],
            ),
        ];
        super::page(
            $page,
            $icon,
            Some("desktop"),
            Body::Sections {
                sections,
                drawers: Vec::new(),
            },
        )
    }};
}

pub(super) fn panel() -> Page {
    bar!("panel", "top", "panel-top", "panel_applets")
}

pub(super) fn dock() -> Page {
    bar!("dock", "bottom", "panel-bottom", "dock_applets")
}
