// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.5 `workspaces.*`.
use super::{DefaultValue as D, Domain::*, KeyDef as K, Kind, Kind::*};

const fn k(path: &'static str, kind: Kind, default: D) -> K {
    K::new(path, kind, default).on("desktop/workspaces", &[Shell])
}

pub(super) const KEYS: &[K] = &[
    k(
        "workspaces.mode",
        Enum(&["span", "per_display"]),
        D::Text("span"),
    ),
    k(
        "workspaces.orientation",
        Enum(&["horizontal", "vertical"]),
        D::Text("horizontal"),
    ),
    k(
        "workspaces.overview_typing",
        Enum(&["none", "launcher", "applications"]),
        D::Text("none"),
    ),
    k("workspaces.wraparound", Bool, D::Bool(true)),
];
