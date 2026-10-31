// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The headless human seat keeps its keyboard, so a window can take focus.
//! Without a GPU: the real protocol engine through testkit, with the seat
//! stripped exactly as `headless::wire::wire` strips it.
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::seat::WaylandFocus;
use testkit::Harness;

#[test]
fn headless_human_seat_keeps_a_keyboard_so_a_window_takes_focus() {
    let mut h = Harness::new();
    headless::wire::human_seat_without_sources(&mut h.wire.state.seat.seat);
    let seat = h.wire.state.seat.seat.clone();
    assert!(seat.get_pointer().is_none(), "seat0 has no pointer source");
    assert!(seat.get_touch().is_none(), "seat0 has no touch source");
    let keyboard = seat
        .get_keyboard()
        .expect("seat0 keeps its keyboard: window focus lives there");

    let (_surface, _xdg, _toplevel) = h.mapped_toplevel(64, 48);
    h.roundtrip();
    let window = h.wire.inner.space.state.elements().next().unwrap().clone();
    let server = window.wl_surface().unwrap().into_owned();
    let id = h.wire.inner.comp.id_for_surface(&server).expect("a record");

    // What `comp.window.focus` does (`focus_keyboard`), then its read-back.
    keyboard.set_focus(
        &mut h.wire.state,
        Some(server.clone()),
        SERIAL_COUNTER.next_serial(),
    );
    h.roundtrip();
    let focused = keyboard
        .current_focus()
        .and_then(|surface| h.wire.inner.comp.id_for_surface(&surface));
    assert_eq!(focused, Some(id), "comp.window.focus would answer refused");
}
