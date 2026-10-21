// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use std::sync::mpsc;
use ui::UiRegistry;

pub struct SurfaceState {
    // The wgpu GPU context is shared driver data (kernel `UI_CONTEXT`), not
    // per-world. This world owns only its registry, built lazily from it.
    pub registry: Option<UiRegistry>,

    pub test_dmabuf_done: bool,
    pub test_example_done: bool,

    pub surface_message_buffer: Vec<crate::surface::protocol::SurfaceMessage>,
    pub surface_message_buffer_channel: (
        mpsc::Sender<crate::surface::protocol::SurfaceMessage>,
        mpsc::Receiver<crate::surface::protocol::SurfaceMessage>,
    ),
}

impl Default for SurfaceState {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceState {
    pub fn new() -> Self {
        Self {
            surface_message_buffer: vec![],
            surface_message_buffer_channel: mpsc::channel(),
            registry: None,
            test_dmabuf_done: false,
            test_example_done: false,
        }
    }
}
