// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Mode listing + the diagnostics dump (ex wire.rs mode logging).

use smithay::reexports::drm::control::{Mode as DrmMode, connector};

pub fn modes(info: &connector::Info) -> &[DrmMode] {
    info.modes()
}

pub fn dump(info: &connector::Info) {
    for (i, m) in modes(info).iter().enumerate() {
        info!(
            "  available mode[{}]: {}x{} @ {}Hz, type: {:?}",
            i,
            m.size().0,
            m.size().1,
            m.vrefresh(),
            m.mode_type(),
        );
    }
}
