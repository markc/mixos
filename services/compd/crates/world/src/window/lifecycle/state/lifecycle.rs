use crate::window::lifecycle::event::event::WindowLifecycleEvent;
use smithay::desktop::Window;

pub struct WindowLifecycle {
    pub incoming: Vec<WindowLifecycleEvent>,
}

impl Default for WindowLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowLifecycle {
    pub fn new() -> Self {
        Self { incoming: vec![] }
    }
}
