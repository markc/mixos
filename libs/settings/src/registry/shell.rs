// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.4 `shell.panels.<panel>.*` (Desktop › Panel, Dock): `top` is COSMIC's
//! Panel, `bottom` its Dock and Quoin's panel.
use super::{DefaultValue as D, Domain::*, Grammar, KeyDef as K, Kind, Kind::*};

/// `shell.panels.<panel>.thickness`: the five sizes (COSMIC's XS to XL), in
/// pixels (F9).
pub const PANEL_THICKNESS_STEPS: &[i64] = &[32, 40, 56, 64, 80];

/// The step nearest `px`, ties up (F9): what a free size becomes.
pub fn snap_panel_thickness(px: u32) -> u32 {
    let px = i64::from(px);
    let mut best = PANEL_THICKNESS_STEPS[0];
    for &step in PANEL_THICKNESS_STEPS {
        if px.abs_diff(step) <= px.abs_diff(best) {
            best = step;
        }
    }
    best as u32
}

const fn k(path: &'static str, kind: Kind, default: D) -> K {
    K::new(path, kind, default).on("desktop/panels", &[Shell])
}
const fn applets(path: &'static str) -> K {
    k(
        path,
        List(&Str(Grammar::Applet, 64), 0, 32),
        D::Owner("Quoin's current order"),
    )
}

pub(super) const KEYS: &[K] = &[
    k(
        "shell.panels.<panel>.enabled",
        Bool,
        D::ByCapture(&[("top", D::Bool(false)), ("bottom", D::Bool(true))]),
    )
    .constraints(&["panel_edges"]),
    k(
        "shell.panels.<panel>.edge",
        Enum(&["top", "bottom", "left", "right"]),
        D::ByCapture(&[("top", D::Text("top")), ("bottom", D::Text("bottom"))]),
    )
    .constraints(&["panel_edges"]),
    k(
        "shell.panels.<panel>.mode",
        Enum(&["dock", "overlay"]),
        D::Text("dock"),
    ),
    k("shell.panels.<panel>.autohide", Bool, D::Bool(false)),
    k(
        "shell.panels.<panel>.output",
        ConnectorOr(&["all"]),
        D::Text("all"),
    )
    .constraints(&["C7"]),
    k("shell.panels.<panel>.gap", Bool, D::Bool(false)),
    k("shell.panels.<panel>.extend", Bool, D::Bool(true)),
    k(
        "shell.panels.<panel>.appearance",
        Enum(&["match", "light", "dark"]),
        D::Text("match"),
    ),
    k(
        "shell.panels.<panel>.thickness",
        OneOf(PANEL_THICKNESS_STEPS),
        D::ByCapture(&[("top", D::Int(32)), ("bottom", D::Int(40))]),
    ),
    k(
        "shell.panels.<panel>.opacity",
        Number {
            min: 0.0,
            max: 1.0,
            step: 0.0,
        },
        D::Number(1.0),
    ),
    applets("shell.panels.<panel>.applets.start"),
    applets("shell.panels.<panel>.applets.center"),
    applets("shell.panels.<panel>.applets.end"),
];
