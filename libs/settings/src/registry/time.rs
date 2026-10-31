// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.12 `time.*`, `region.*`. Zone, NTP and region are choices, applied by
//! each machine's hostd; the migration takes them from the host.
use super::{DefaultValue as D, Domain::*, Grammar, KeyDef as K, Kind, Kind::*};

const DATE_TIME: &str = "time-language/date-time";
const REGION: &str = "time-language/region";
const LOCALE: Kind = Str(Grammar::Locale, 64);

const fn clock(path: &'static str, kind: Kind, default: D) -> K {
    K::new(path, kind, default).on(DATE_TIME, &[Shell])
}

pub(super) const KEYS: &[K] = &[
    K::new(
        "time.zone",
        Str(Grammar::Zone, 64),
        D::Owner("the host's zone at migration"),
    )
    .on(DATE_TIME, &[System])
    .applied("hostd"),
    K::new("time.ntp", Bool, D::Bool(true))
        .on(DATE_TIME, &[System])
        .applied("hostd"),
    clock("time.format.24h", Bool, D::Null),
    clock(
        "time.format.first_day",
        Enum(&["monday", "friday", "saturday", "sunday"]),
        D::Null,
    ),
    clock("time.applet.show_date", Bool, D::Bool(true)),
    clock("time.applet.show_weekday", Bool, D::Bool(false)),
    clock("time.applet.show_seconds", Bool, D::Bool(false)),
    K::new(
        "region.languages",
        List(&LOCALE, 1, 8),
        D::Owner("the host's languages at migration"),
    )
    .on(REGION, &[System])
    .applied("hostd"),
    K::new(
        "region.formats",
        LOCALE,
        D::Owner("the host's formats at migration"),
    )
    .on(REGION, &[System])
    .applied("hostd"),
];
