// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.10 `keymaps.*`. `keymaps.comp.*` are the compositor's bindings, where
//! a bare F1–F12 is refused (`user_keys`).
use super::{DefaultValue as D, Domain::*, KeyDef as K, Kind::*};

pub(super) const KEYS: &[K] = &[
    K::new("keymaps.<app>.<action>", Chords, D::Owner("the app's own"))
        .on("input/keyboard/shortcuts", &[Input])
        .constraints(&["C8", "user_keys"]),
    K::new("keymaps.comp.spawn.<name>", Spawn, D::Null)
        .on("input/keyboard/shortcuts", &[Input])
        .constraints(&["C8", "user_keys"]),
];
