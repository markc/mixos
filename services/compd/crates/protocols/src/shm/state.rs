// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use smithay::wayland::shm::ShmState;

pub struct SHMState {
    // Manages `wl_shm`. Allows clients to share CPU-allocated memory buffers containing pixel data.
    pub state: ShmState,
}
