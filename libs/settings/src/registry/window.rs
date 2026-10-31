// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.3 `window.*` (Desktop › Window management: chrome and behaviour). The
//! last eight are the framing keys moved from `appearance.*`.
use super::{Alpha, DefaultValue as D, Domain, Domain::*, KeyDef as K, Kind, Kind::*};

const CHROME: &[Domain] = &[Paint, Layout];

const fn k(path: &'static str, kind: Kind, default: D, domains: &'static [Domain]) -> K {
    K::new(path, kind, default).on("desktop/window-management", domains)
}
const fn behaviour(path: &'static str) -> K {
    k(path, Bool, D::Bool(false), &[Input])
}

pub(super) const KEYS: &[K] = &[
    k("window.hint_shown", Bool, D::Bool(true), &[Paint]),
    k(
        "window.active_hint",
        Int {
            min: 0,
            max: 64,
            step: 1,
        },
        D::Int(3),
        CHROME,
    ),
    k(
        "window.hint_colour",
        ColourOr(&["accent"], Alpha::Opaque),
        D::Text("accent"),
        &[Paint],
    ),
    k(
        "window.gaps",
        Int {
            min: 0,
            max: 500,
            step: 1,
        },
        D::Int(0),
        &[Layout],
    ),
    k("window.controls.minimize", Bool, D::Bool(true), CHROME),
    k("window.controls.maximize", Bool, D::Bool(true), CHROME),
    k("window.clip_floating", Bool, D::Bool(true), &[Paint]),
    k("window.clip_tiled", Bool, D::Bool(false), &[Paint]),
    k("window.shadow_tiled", Bool, D::Bool(false), &[Paint]),
    k(
        "window.super_key",
        Enum(&["launcher", "workspaces", "applications", "none"]),
        D::Text("launcher"),
        &[Input],
    ),
    behaviour("window.edge_gravity"),
    behaviour("window.focus_follows_cursor"),
    k(
        "window.focus_delay_ms",
        Int {
            min: 0,
            max: 2000,
            step: 1,
        },
        D::Int(250),
        &[Input],
    )
    .used_when("window.focus_follows_cursor")
    .constraints(&["C2"]),
    behaviour("window.cursor_follows_focus"),
    k(
        "window.decorations",
        Enum(&["auto", "csd", "ssd"]),
        D::Text("auto"),
        CHROME,
    )
    .mixos(),
    k(
        "window.caption_side",
        Known(design::CaptionSide::NAMES),
        D::Text("auto"),
        CHROME,
    )
    .mixos(),
    k(
        "window.style",
        Known(design::WindowStyle::NAMES),
        D::Text("mac"),
        CHROME,
    )
    .mixos(),
    k(
        "window.menu_style",
        Known(design::MenuStyle::NAMES),
        D::Text("bar"),
        CHROME,
    )
    .mixos(),
    k(
        "window.menu_side",
        Known(design::MenuSide::NAMES),
        D::Text("left"),
        CHROME,
    )
    .mixos(),
    k(
        "window.actions_place",
        Known(design::ActionsPlace::NAMES),
        D::Text("row"),
        CHROME,
    )
    .mixos(),
    k(
        "window.actions_align",
        Known(design::ActionsAlign::NAMES),
        D::Text("right"),
        CHROME,
    )
    .mixos(),
    k(
        "window.title_align",
        Known(design::TitleAlign::NAMES),
        D::Text("auto"),
        CHROME,
    )
    .mixos(),
];
