// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

pub mod backend {
    pub use crate::display::backend::*;
}

use crate::display::backend::Backend;
use render_gles::format::registrar;
use render_gles::format::role::Role;
use smithay::output::Output;
use world::state::Loop;

pub fn register(_loop: &mut Loop, output: &Output) {
    let _global =
        output.create_global::<dispatcher::state::Dispatch>(&_loop.state.output.display_handle);

    // Every world's Space, not just the hosted one — a world parked when a monitor
    // appears would otherwise never learn about it (`map_output_everywhere`).
    _loop
        .inner
        .map_output_everywhere(output, smithay::utils::Point::from((0, 0)));
}

/// Bind the EGL display (so smithay can import client dmabufs) and REGISTER what
/// it can take.
///
/// Registration only — the advertisement that used to follow it lives in
/// [`advertise_dmabuf`] now. They were one function, and that put an ANSWER in
/// the middle of the registration phase: the dmabuf feedback was built here,
/// while the wgpu adapters were still probing on their own thread, so it was
/// computed from a registrar that was not finished. See `Registrar::expect`.
pub fn bind_display(_loop: &mut Loop, backend_loader: &mut dyn Backend) {
    let egl_formats = backend_loader.bind_display(&_loop.state.output.display_handle);
    // The EGL import list is a DEVICE capability, registered like any other. The
    // advertisement's fallback and the log's baseline, NOT an intersection term.
    let formats = _loop
        .inner
        .kernel
        .get(&render_gles::format::registrar::FORMATS)
        .clone();
    formats.register(
        registrar::Device::UNSPECIFIED,
        Role::GlesSample,
        egl_formats,
        "egl (bind_display)",
    );
}
