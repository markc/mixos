// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use smithay::{
    desktop::{Space, Window},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
};

// Interactive resize commit handling is currently disabled.
pub fn handle_commit(_space: &mut Space<Window>, _surface: &WlSurface) -> Option<()> {
    None
}
