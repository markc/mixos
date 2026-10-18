// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
pub mod wire {
    pub use crate::wayland::compositor::place::WindowPlacedMarker;
    pub use crate::wayland::compositor::place::handle_commit;
    pub use crate::wayland::compositor::session::{
        apply_commit, awaits_initial_configure, commit, compositor_state,
    };
    pub use protocols::compositor::client::client::client_compositor_state;
}
