// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Desktop › Window management (spec 1 §4.4; the matrix §4): the Super key,
//! floating windows, the window controls and focus navigation. The window
//! chrome additions (decorations, buttons, menus, titles) are MixOS's own
//! and sit under Window controls with the rest.
use super::{Body, ChoiceStyle, Cond, Control, Opt, Page, opt, row, section, untitled};

const SUPER_KEY: &[Opt] = &[
    opt("launcher", "opt-super-launcher"),
    opt("workspaces", "opt-super-workspaces"),
    opt("applications", "opt-super-applications"),
    opt("none", "opt-super-none"),
];
const DECORATIONS: &[Opt] = &[
    opt("auto", "look-align-auto"),
    opt("csd", "opt-client-side"),
    opt("ssd", "opt-server-side"),
];
const CAPTION_SIDE: &[Opt] = &[
    opt("auto", "look-align-auto"),
    opt("left", "opt-left"),
    opt("center", "look-menu-side-center"),
    opt("right", "opt-right"),
];
const WINDOW_STYLE: &[Opt] = &[
    opt("mac", "look-window-style-mac"),
    opt("win11", "look-window-style-win11"),
    opt("mixos", "look-window-style-mixos"),
];
const MENU_STYLE: &[Opt] = &[
    opt("bar", "look-menu-style-bar"),
    opt("button", "look-menu-style-button"),
    opt("titlebar", "look-menu-style-titlebar"),
];
const MENU_SIDE: &[Opt] = &[
    opt("left", "look-menu-side-left"),
    opt("center", "look-menu-side-center"),
    opt("right", "look-menu-side-right"),
];
const ACTIONS_PLACE: &[Opt] = &[
    opt("row", "look-actions-place-row"),
    opt("titlebar", "look-actions-place-titlebar"),
];
const ACTIONS_ALIGN: &[Opt] = &[
    opt("left", "look-menu-side-left"),
    opt("center", "look-menu-side-center"),
    opt("right", "look-menu-side-right"),
];
const TITLE_ALIGN: &[Opt] = &[
    opt("auto", "look-align-auto"),
    opt("left", "look-menu-side-left"),
    opt("center", "look-menu-side-center"),
    opt("right", "look-menu-side-right"),
];

/// The delay is only read while focus follows the cursor (constraint C2).
const DELAY_RULES: &[(Cond, &str)] = &[(
    Cond::True("window.focus_follows_cursor"),
    "row-window_management-focus_delay-off",
)];

pub(super) fn page() -> Page {
    let first = untitled(
        "first",
        vec![
            row(
                "super_key",
                Control::Choice {
                    key: "window.super_key",
                    style: ChoiceStyle::Dropdown,
                    options: SUPER_KEY,
                },
            ),
            row(
                "edge_gravity",
                Control::Toggle {
                    key: "window.edge_gravity",
                },
            ),
        ],
    );
    let controls = section(
        "controls",
        vec![
            row(
                "hint_shown",
                Control::Toggle {
                    key: "window.hint_shown",
                },
            ),
            row(
                "maximize",
                Control::Toggle {
                    key: "window.controls.maximize",
                },
            ),
            row(
                "minimize",
                Control::Toggle {
                    key: "window.controls.minimize",
                },
            ),
            row(
                "decorations",
                Control::Choice {
                    key: "window.decorations",
                    style: ChoiceStyle::Dropdown,
                    options: DECORATIONS,
                },
            ),
            row(
                "caption_side",
                Control::Choice {
                    key: "window.caption_side",
                    style: ChoiceStyle::Dropdown,
                    options: CAPTION_SIDE,
                },
            ),
            row(
                "style",
                Control::Choice {
                    key: "window.style",
                    style: ChoiceStyle::Dropdown,
                    options: WINDOW_STYLE,
                },
            ),
            row(
                "menu_style",
                Control::Choice {
                    key: "window.menu_style",
                    style: ChoiceStyle::Dropdown,
                    options: MENU_STYLE,
                },
            ),
            row(
                "menu_side",
                Control::Choice {
                    key: "window.menu_side",
                    style: ChoiceStyle::Dropdown,
                    options: MENU_SIDE,
                },
            ),
            row(
                "actions_place",
                Control::Choice {
                    key: "window.actions_place",
                    style: ChoiceStyle::Dropdown,
                    options: ACTIONS_PLACE,
                },
            ),
            row(
                "actions_align",
                Control::Choice {
                    key: "window.actions_align",
                    style: ChoiceStyle::Dropdown,
                    options: ACTIONS_ALIGN,
                },
            ),
            row(
                "title_align",
                Control::Choice {
                    key: "window.title_align",
                    style: ChoiceStyle::Dropdown,
                    options: TITLE_ALIGN,
                },
            ),
        ],
    );
    let focus = section(
        "focus",
        vec![
            row(
                "focus_follows_cursor",
                Control::Toggle {
                    key: "window.focus_follows_cursor",
                },
            )
            .backend("backend-compd-focus"),
            row(
                "focus_delay",
                Control::NumberEntry {
                    key: "window.focus_delay_ms",
                    min: 0,
                    max: 2000,
                },
            )
            .enable(DELAY_RULES)
            .backend("backend-compd-focus"),
            row(
                "cursor_follows_focus",
                Control::Toggle {
                    key: "window.cursor_follows_focus",
                },
            )
            .backend("backend-compd-pointer-warp"),
        ],
    );
    super::page(
        "window_management",
        "app-window",
        Some("desktop"),
        Body::Sections {
            sections: vec![first, controls, focus],
            drawers: Vec::new(),
        },
    )
}
