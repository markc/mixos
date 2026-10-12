use std::sync::mpsc;
use ui::{HandleId, UiHandle, UiRegistry};

pub struct SurfaceState {
    // The wgpu GPU context is shared driver data (kernel `UI_CONTEXT`), not
    // per-world. This world owns only its registry, built lazily from it.
    pub registry: Option<UiRegistry>,

    pub test_dmabuf_done: bool,
    pub test_example_done: bool,

    pub surface_message_buffer: Vec<crate::surface::protocol::protocol::SurfaceMessage>,
    pub surface_message_buffer_channel: (
        mpsc::Sender<crate::surface::protocol::protocol::SurfaceMessage>,
        mpsc::Receiver<crate::surface::protocol::protocol::SurfaceMessage>,
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
