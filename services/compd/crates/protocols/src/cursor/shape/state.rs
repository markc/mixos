// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use smithay::wayland::cursor_shape::CursorShapeManagerState;

pub struct CursorShape {
    pub state: CursorShapeManagerState,
}
