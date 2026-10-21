// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use std::collections::HashSet;

use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufState};
use smithay::wayland::drm_syncobj::DrmSyncobjState;

pub struct DMABufState {
    /// The global advertisement for hardware-accelerated buffers.
    /// Without this, clients like Alacritty will fall back to software rendering
    /// or fail to launch.
    pub global: Option<DmabufGlobal>,

    /// Tracks the state of all GPU-resident memory buffers currently
    /// shared between the compositor and apps.
    pub state: DmabufState,

    // This is in DMABuf state because it should be directly related to Dmabufs.
    pub syncobj_state: Option<DrmSyncobjState>,

    pub syncobj_hook_installed: HashSet<ObjectId>,
}
