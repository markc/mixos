// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.11 `apps_default.*`, `apps_startup.*` (applied by xdg-export, which
//! owns `mimeapps.list` and the autostart directory) and `xwayland.*`.
use super::{DefaultValue as D, Domain::*, Grammar, KeyDef as K, Kind, Kind::*};

const DESKTOP_ID: Kind = Str(Grammar::DesktopId, 255);

const fn xwayland(path: &'static str, kind: Kind, default: D) -> K {
    K::new(path, kind, default).on("applications/x11", &[Input])
}

pub(super) const KEYS: &[K] = &[
    K::new("apps_default.<category>", DESKTOP_ID, D::Null)
        .on("applications/default", &[System])
        .applied("xdg-export"),
    K::new(
        "apps_startup.entries",
        List(&DESKTOP_ID, 0, 64),
        D::Json("[]"),
    )
    .on("applications/startup", &[System])
    .applied("xdg-export"),
    xwayland(
        "xwayland.global_shortcuts",
        Enum(&["none", "modifiers", "combinations", "all"]),
        D::Text("none"),
    ),
    xwayland("xwayland.mouse_events", Bool, D::Bool(false)),
    xwayland(
        "xwayland.scaling",
        Enum(&["gaming", "applications", "compatibility"]),
        D::Text("applications"),
    ),
    xwayland(
        "xwayland.primary_output",
        ConnectorOr(&["none"]),
        D::Text("none"),
    ),
];
