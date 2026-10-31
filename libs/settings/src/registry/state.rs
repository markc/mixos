// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.17 internal state keys: stored and sealed (the resolved mode depends
//! on them), never shown in Prefs.
use super::{DefaultValue as D, Domain::*, KeyDef as K, Kind::*};

pub(super) const KEYS: &[K] = &[
    K::new("state.appearance.daylight", Bool, D::Null)
        .domains(&[Paint])
        .state("daylight"),
    K::new(
        "state.appearance.mode_override",
        Enum(&["dark", "light"]),
        D::Null,
    )
    .domains(&[Paint])
    // Prefs writes it while the mode is auto; the daylight citizen clears
    // it at the next sunrise or sunset.
    .state("prefs"),
];
