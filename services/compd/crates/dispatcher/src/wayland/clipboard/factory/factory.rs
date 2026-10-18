// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::state::state::DispatchWire;
use protocols::clipboard::state::Clipboard;
use smithay::reexports::wayland_server::protocol::wl_data_device_manager::WlDataDeviceManager;
use smithay::reexports::wayland_server::{DisplayHandle, GlobalDispatch};
use smithay::wayland::GlobalData;
use smithay::wayland::selection::data_device::{DataDeviceHandler, DataDeviceState};

pub fn new<I>(display_handle: &DisplayHandle) -> Clipboard
where
    I: DispatchWire,
    I: GlobalDispatch<WlDataDeviceManager, GlobalData> + 'static,
    I: DataDeviceHandler,
{
    // Initialize Clipboard and Drag-and-Drop functionality.
    // Side-effect: Interacts with `calloop` heavily when users highlight text or drag files,
    // negotiating MIME types between two separate Wayland clients asynchronously.
    let data_device_state = DataDeviceState::new::<I>(display_handle);

    Clipboard {
        data_device_state,
        capture: Default::default(),
        // Started lazily by the drain on the first copy — it needs a loop handle.
        worker: None,
        pending_capture: None,
        pending_sends: vec![],
        pending_focus: None,
    }
}
