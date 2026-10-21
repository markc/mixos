// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use std::collections::hash_set;

use crate::state::DispatchWire;
use protocols::dmabuf::state::DMABufState;
use smithay::{
    backend::drm::DrmDeviceFd,
    reexports::{
        wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1,
        wayland_server::{DisplayHandle, GlobalDispatch},
    },
    wayland::{dmabuf::DmabufState, drm_syncobj::DrmSyncobjGlobalData},
};

pub fn new<I>(_display_handle: &DisplayHandle, _drm_device: Option<DrmDeviceFd>) -> DMABufState
where
    I: DispatchWire,
    I: GlobalDispatch<WpLinuxDrmSyncobjManagerV1, DrmSyncobjGlobalData>,
    I: 'static,
{
    let dmabuf_state = DmabufState::new();

    DMABufState {
        state: dmabuf_state,
        global: None, // Empty until the GPU starts
        syncobj_state: None,
        syncobj_hook_installed: hash_set::HashSet::new(),
    }
}
