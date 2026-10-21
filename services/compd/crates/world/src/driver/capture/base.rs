// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::capture::session::CaptureState;
use graphics::capture::registry::CaptureRegistry;
use slots::storage::token::base::{Token, TokenMut};

/// Capture driver data: the capture registry + session state live in the
/// kernel/driver storage by token, not as Orchestrator fields.
pub static CAPTURE_REGISTRY: Token<Option<CaptureRegistry>> = Token::new();
pub static CAPTURE_REGISTRY_MUT: TokenMut<Option<CaptureRegistry>> =
    TokenMut::new(&CAPTURE_REGISTRY);
pub static CAPTURE: Token<CaptureState> = Token::new();
pub static CAPTURE_MUT: TokenMut<CaptureState> = TokenMut::new(&CAPTURE);
