// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

pub struct DNDState {
    pub icon: Option<WlSurface>,
}
