// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::seat::pointer::state::PointerState;
use slots::storage::token::base::{Token, TokenMut};
use slots::trait_::system::base::{System, WorldBuilder};

pub static POINTER: Token<PointerState> = Token::new();
/// TRANSITIONAL pub: legacy call sites still write this slot directly until
/// their logic moves into systems/events.
pub static POINTER_MUT: TokenMut<PointerState> = TokenMut::new(&POINTER);

/// Owns the pointer slot.
#[derive(Default)]
pub struct PointerSystem;

impl System for PointerSystem {
    fn name(&self) -> &'static str {
        "pointer"
    }

    fn register(&mut self, builder: &mut WorldBuilder) {
        builder.storage.insert(&POINTER, PointerState::new());
    }
}
