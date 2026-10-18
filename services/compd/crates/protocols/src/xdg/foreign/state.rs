// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::wayland::shell::xdg::decoration::{XdgDecorationHandler, XdgDecorationState};
use smithay::wayland::xdg_foreign::XdgForeignState;

pub struct Foreign {
    pub xdg_foreign_state: XdgForeignState,
}
