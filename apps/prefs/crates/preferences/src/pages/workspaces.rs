// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Desktop › Workspaces (spec 1 §4.5; the matrix §5): what typing does in
//! the overview, whether displays share one set of workspaces, the
//! orientation of the switcher, and wrap-around navigation. The overview
//! typing action and per-display workspaces are compd's, so those two rows
//! are drawn disabled naming the backend; the other two are live.
use super::{ChoiceStyle, Control, Opt, Page, opt, row, section};

const TYPING: &[Opt] = &[
    opt("none", "opt-none"),
    opt("launcher", "opt-open-launcher"),
    opt("applications", "opt-open-applications"),
];
const MODES: &[Opt] = &[
    opt("span", "opt-span-displays"),
    opt("per_display", "opt-separate-displays"),
];
const ORIENTATIONS: &[Opt] = &[
    opt("vertical", "opt-vertical"),
    opt("horizontal", "opt-horizontal"),
];

pub(super) fn page() -> Page {
    super::page(
        "workspaces",
        "layers",
        Some("desktop"),
        super::sections(vec![
            section(
                "overview",
                vec![
                    row(
                        "typing",
                        Control::Choice {
                            key: "workspaces.overview_typing",
                            style: ChoiceStyle::Dropdown,
                            options: TYPING,
                        },
                    )
                    .backend("backend-compd-overview"),
                ],
            ),
            section(
                "multi_monitor",
                vec![
                    row(
                        "mode",
                        Control::Choice {
                            key: "workspaces.mode",
                            style: ChoiceStyle::Radio,
                            options: MODES,
                        },
                    )
                    .backend("backend-compd-per-output"),
                ],
            ),
            section(
                "orientation",
                vec![row(
                    "orientation",
                    Control::Choice {
                        key: "workspaces.orientation",
                        style: ChoiceStyle::Radio,
                        options: ORIENTATIONS,
                    },
                )],
            ),
            section(
                "navigation",
                vec![row(
                    "wraparound",
                    Control::Toggle {
                        key: "workspaces.wraparound",
                    },
                )],
            ),
        ]),
    )
}
