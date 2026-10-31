// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.13 `system.*`: live, per machine, through hostd on the machine the
//! desk runs on.
use super::KeyDef as K;

pub(super) const KEYS: &[K] = &[
    K::live("system.hostname", "string", "hostd")
        .page("system/about")
        .reads("host.info"),
    K::live("system.users", "records", "hostd").page("system/users"),
    K::live("system.firmware", "records", "hostd").page("system/firmware"),
    K::live("system.about", "records", "hostd")
        .page("system/about")
        .reads("host.info"),
];
