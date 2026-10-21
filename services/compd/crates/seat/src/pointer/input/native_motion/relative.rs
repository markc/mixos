// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::pointer::input::native_motion::dispatch;
use smithay::backend::input::{Event, InputBackend};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use world::state::Loop;

pub fn input_received_normalized<I: InputBackend>(
    event: &I::PointerMotionEvent,
    _loop: &mut Loop,
    position_normalized: Point<f64, Logical>,
    _position_screen: &Point<f64, Logical>,
    delta: (Point<f64, Logical>, Point<f64, Logical>),
    was_constrain_locked: bool,
) {
    let serial = SERIAL_COUNTER.next_serial();
    let pointer = _loop.state.seat.seat.get_pointer().unwrap();

    dispatch::dispatch(
        _loop,
        event.time_msec(),
        serial,
        pointer,
        position_normalized,
        Some(delta),
        was_constrain_locked,
    );
}
