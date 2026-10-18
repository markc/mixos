// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::xdg_activation::XdgActivationState;

pub struct Activation {
    pub xdg_activation: XdgActivationState,
}
