// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::state::DispatchWire;
use protocols::xdg::foreign::state::Foreign;
use smithay::reexports::wayland_protocols::xdg::foreign::zv2::server::zxdg_exporter_v2::ZxdgExporterV2;
use smithay::reexports::wayland_protocols::xdg::foreign::zv2::server::zxdg_importer_v2::ZxdgImporterV2;
use smithay::reexports::wayland_server::{DisplayHandle, GlobalDispatch};
use smithay::wayland::GlobalData;
use smithay::wayland::xdg_foreign::{XdgForeignHandler, XdgForeignState};
pub fn new<I>(display_handle: &DisplayHandle) -> Foreign
where
    I: DispatchWire,
    I: XdgForeignHandler,
    I: GlobalDispatch<ZxdgExporterV2, GlobalData>,
    I: GlobalDispatch<ZxdgImporterV2, GlobalData>,
{
    let xdg_foreign_state = XdgForeignState::new::<I>(display_handle);

    Foreign { xdg_foreign_state }
}
