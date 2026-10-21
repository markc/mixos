// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Computes the rim-facing `DisplaySnapshot` (external present? internal panel
//! active?) from a live connector scan. The kernel owns the DRM device; the rim
//! reads only this primitive summary via the lid driver token.

use drivers::lid::base::DisplaySnapshot;
use kms::connector::kind::{ConnectorKind, classify};
use smithay::backend::drm::DrmDevice;
use smithay::reexports::drm::control::connector;

/// Scan all connectors on `drm` and summarize for the lid policy. `active` is the
/// connector currently driving the output (its kind decides `internal_active`).
pub fn compute(drm: &DrmDevice, active: connector::Handle) -> DisplaySnapshot {
    let res = kms::connector::scan::resources(drm);
    let infos = kms::connector::scan::connectors(drm, &res);

    let mut snapshot = DisplaySnapshot::default();
    for info in &infos {
        let kind = classify(info);
        let connected = info.state() == connector::State::Connected;
        if connected && kind == ConnectorKind::External {
            snapshot.external_present = true;
        }
        if info.handle() == active && kind == ConnectorKind::InternalPanel {
            snapshot.internal_active = true;
        }
    }
    snapshot
}
