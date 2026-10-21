// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::capture::session::message::CaptureMessage;

// The screen-capture panel is the one UI producer of surface messages.

#[derive(Debug)]
pub struct SurfaceMessage {
    // SurfaceID // <-- by HandlerID is available just in case its needed
    pub message: SurfaceMessageType,
}

#[derive(Debug)]
pub enum SurfaceMessageType {
    Capture(CaptureMessage),
}
