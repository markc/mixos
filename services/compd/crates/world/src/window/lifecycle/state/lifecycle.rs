use crate::window::lifecycle::event::event::WindowLifecycleEvent;
use smithay::desktop::Window;

pub struct WindowLifecycle {
    pub incoming: Vec<WindowLifecycleEvent>,
}

impl WindowLifecycle {
    pub fn new() -> Self {
        return Self { incoming: vec![] };
    }
}
