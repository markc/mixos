// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

pub mod delegate_main {
    pub use crate::delegate::main::delegate_main::*;
}

pub mod gesture;
pub mod main;

use smithay::backend::input::{InputBackend, InputEvent};
use world::state::Loop;


/// Delegation of input events from the compositor seat loop.
///
/// There is no picker or lock-screen route here; the session lock will be
/// ext-session-lock, wired with the comp policy's seat rules.
pub fn process_input_event<I: InputBackend>(_loop: &mut Loop, event: &InputEvent<I>) {
    match _loop.inner.status {
        world::state::Status::Running => {
            delegate_main::process_input_event(_loop, event);
        }
        world::state::Status::Terminate => {}
    }
}
