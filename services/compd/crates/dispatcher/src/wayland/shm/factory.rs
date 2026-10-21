// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::state::DispatchWire;
use protocols::shm::state::SHMState;
use smithay::reexports::wayland_server::protocol::wl_shm::WlShm;
use smithay::reexports::wayland_server::protocol::wl_shm_pool::WlShmPool;
use smithay::reexports::wayland_server::{Dispatch, DisplayHandle, GlobalDispatch};
use smithay::wayland::GlobalData;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::shm::{ShmHandler, ShmPoolUserData, ShmState};

pub fn new<I>(display_handle: &DisplayHandle) -> SHMState
where
    I: DispatchWire,
    I: GlobalDispatch<WlShm, GlobalData>
        + Dispatch<WlShm, GlobalData>
        + Dispatch<WlShmPool, ShmPoolUserData>
        + BufferHandler
        + ShmHandler
        + 'static,
{
    // Initialize Shared Memory protocol.
    // We pass `vec![]` for formats because ARGB8888 and XRGB8888 are supported by default.
    // Side-effect: When clients attach SHM buffers to surfaces, `calloop` handles the memory
    // mapping automatically behind the scenes.
    let shm_state = ShmState::new::<I>(
        display_handle,
        // The extras beyond the protocol's mandatory Argb/Xrgb, from the format
        // layer. Advertising one the renderer cannot upload is a client that gets
        // nothing — which is what `Bgr888` did here for as long as it was listed.
        render_gles::format::answer::shm_extra(),
    );

    SHMState { state: shm_state }
}
