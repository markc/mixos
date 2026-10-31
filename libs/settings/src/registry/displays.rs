// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.6 `displays.<connector>.*`, applied by monitord. settingsd stores only
//! confirmed values: Prefs previews through monitord and writes on Keep.
use super::{DefaultValue as D, Domain::*, Grammar, KeyDef as K, Kind, Kind::*};

const fn k(path: &'static str, kind: Kind, default: D) -> K {
    K::new(path, kind, default)
        .on("system/displays", &[System])
        .applied("monitord")
}

pub(super) const KEYS: &[K] = &[
    k("displays.<connector>.enabled", Bool, D::Bool(true)).constraints(&["C6"]),
    k("displays.<connector>.mirror_of", ConnectorOr(&[]), D::Null).constraints(&["C5"]),
    k(
        "displays.<connector>.mode",
        Str(Grammar::OutputMode, 32),
        D::Null,
    ),
    k(
        "displays.<connector>.vrr",
        Enum(&["always", "auto", "off"]),
        D::Text("auto"),
    ),
    k(
        "displays.<connector>.scale",
        Int {
            min: 50,
            max: 320,
            step: 5,
        },
        D::Null,
    ),
    k(
        "displays.<connector>.rotation",
        Enum(&["normal", "90", "180", "270"]),
        D::Text("normal"),
    ),
    k(
        "displays.<connector>.position",
        Str(Grammar::Position, 32),
        D::Null,
    ),
];
