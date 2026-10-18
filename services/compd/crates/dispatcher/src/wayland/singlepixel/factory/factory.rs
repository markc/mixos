// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::state::state::DispatchWire;
use protocols::singlepixel::state::SinglePixel;
use smithay::{
    reexports::{
        wayland_protocols::wp::single_pixel_buffer::v1::server::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
        wayland_server::{Dispatch, DisplayHandle, GlobalDispatch},
    },
    wayland::{GlobalData, single_pixel_buffer::SinglePixelBufferState},
};

pub fn new<I>(display_handle: &DisplayHandle) -> SinglePixel
where
    I: DispatchWire,
    I: GlobalDispatch<WpSinglePixelBufferManagerV1, GlobalData>,
    I: Dispatch<WpSinglePixelBufferManagerV1, GlobalData>,
    I: 'static,
{
    let single_pixel_buffer_state = SinglePixelBufferState::new::<I>(display_handle);
    SinglePixel {
        single_pixel_buffer_state,
    }
}
