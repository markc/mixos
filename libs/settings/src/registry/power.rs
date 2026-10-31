// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.8 `power.*`.
use super::{DefaultValue as D, Domain::*, KeyDef as K, Kind, Kind::*};

const PAGE: &str = "system/power";

const fn k(path: &'static str, kind: Kind, default: D, owner: &'static str) -> K {
    K::new(path, kind, default)
        .on(PAGE, &[System])
        .applied(owner)
}
/// Suspend after 15 minutes to 2 hours, on a 5-minute grid; null is never.
const SUSPEND: Kind = Int {
    min: 900,
    max: 7200,
    step: 300,
};

pub(super) const KEYS: &[K] = &[
    k(
        "power.profile",
        Enum(&["battery", "balanced", "performance"]),
        D::Text("balanced"),
        "hostd",
    ),
    k(
        "power.screen_off_s",
        OneOf(&[120, 300, 600, 900, 1800]),
        D::Null,
        "compd",
    ),
    k("power.suspend_ac_s", SUSPEND, D::Null, "hostd"),
    k("power.suspend_battery_s", SUSPEND, D::Null, "hostd"),
    K::live("power.battery", "records", "hostd").page(PAGE),
    K::live("power.devices", "records", "hostd").page(PAGE),
];
