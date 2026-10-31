// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.15 `network.*`, `bluetooth.*`: a radio is per machine, so both
//! switches are live. The connection lists are hostd's, not catalogued.
use super::KeyDef as K;

pub(super) const KEYS: &[K] = &[
    K::live("network.wifi.enabled", "bool", "hostd")
        .page("network/wifi")
        .reads("net.state"),
    K::live("bluetooth.enabled", "bool", "hostd").page("bluetooth"),
];
