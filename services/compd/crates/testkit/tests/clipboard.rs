// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The clipboard through compd's real selection handlers: the offer follows
//! keyboard focus, a copy survives its source going away, a copy made on the
//! agent seat is offered on the primary seat, and an unfocused client cannot
//! take the clipboard.

use testkit::Harness;
use testkit::clipboard::ClipboardRead;

const TEXT: &str = "text/plain;charset=utf-8";
const PRIMARY: usize = 0;
const AGENT: usize = 1;

/// Pump until the paste's pipe closes, and return what it carried.
fn finish(h: &mut Harness, read: &mut ClipboardRead) -> Vec<u8> {
    for _ in 0..400 {
        h.pump();
        if read.poll() {
            return read.bytes().to_vec();
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the paste never finished");
}

/// The main client (copying) and one extra client (pasting), both with data
/// devices on every seat; the paster has a surface the keyboard can focus.
fn two_clients() -> (Harness, usize, wayland_client::protocol::wl_surface::WlSurface) {
    let mut h = Harness::new();
    h.client.bind_data_devices();
    h.roundtrip();
    assert_eq!(h.client.seat_count(), 2, "the primary seat and the agent seat");
    let b = h.add_client();
    h.extra_clients[b].bind_data_devices();
    let surface = h.extra_clients[b].create_surface();
    h.extra_clients[b].commit(&surface);
    h.roundtrip_client(b);
    (h, b, surface)
}

#[test]
fn the_clipboard_is_offered_to_the_focused_client() {
    let (mut h, b, paster) = two_clients();
    let (copier, ..) = h.mapped_toplevel(64, 48);
    h.focus_keyboard(&copier);
    h.client.set_selection(PRIMARY, &[TEXT], b"alpha");
    h.roundtrip();
    assert!(
        h.extra_clients[b].selection_mimes(PRIMARY).is_none(),
        "an unfocused client is offered nothing"
    );
    h.focus_keyboard_client(b, &paster);
    h.roundtrip_client(b);
    let mimes = h.extra_clients[b]
        .selection_mimes(PRIMARY)
        .expect("the focused client is offered the clipboard");
    assert!(mimes.iter().any(|m| m == TEXT));
    let mut read = h.extra_clients[b].receive(PRIMARY, TEXT);
    assert_eq!(finish(&mut h, &mut read), b"alpha");
}

#[test]
fn a_copy_survives_its_source_going_away() {
    let (mut h, b, paster) = two_clients();
    let (copier, ..) = h.mapped_toplevel(64, 48);
    h.focus_keyboard(&copier);
    let source = h.client.set_selection(PRIMARY, &[TEXT], b"bravo");
    // Let the capture read the copy while its owner is alive.
    for _ in 0..20 {
        h.pump();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    source.destroy();
    h.roundtrip();
    let served = h.client.state.clipboard.served.len();
    h.focus_keyboard_client(b, &paster);
    h.roundtrip_client(b);
    let mut read = h.extra_clients[b].receive(PRIMARY, TEXT);
    assert_eq!(finish(&mut h, &mut read), b"bravo");
    assert_eq!(
        h.client.state.clipboard.served.len(),
        served,
        "served from compd's copy, not by the gone source"
    );
}

#[test]
fn an_agent_seat_copy_is_offered_on_the_primary_seat() {
    let (mut h, b, paster) = two_clients();
    let (copier, ..) = h.mapped_toplevel(64, 48);
    h.focus_agent_keyboard(&copier);
    let source = h.client.set_selection(AGENT, &[TEXT], b"charlie");
    h.roundtrip();
    h.focus_keyboard_client(b, &paster);
    h.roundtrip_client(b);
    let mimes = h.extra_clients[b]
        .selection_mimes(PRIMARY)
        .expect("the agent seat's copy is offered on the primary seat");
    assert!(mimes.iter().any(|m| m == TEXT));
    // Live: the agent-seat client answers.
    let mut read = h.extra_clients[b].receive(PRIMARY, TEXT);
    assert_eq!(finish(&mut h, &mut read), b"charlie");
    assert!(
        h.client
            .state
            .clipboard
            .served
            .iter()
            .any(|(_, m)| m == TEXT),
        "the agent-seat client served the paste"
    );
    // And it survives that client's source going away, like any copy.
    source.destroy();
    h.roundtrip();
    h.roundtrip_client(b);
    assert!(
        h.extra_clients[b].selection_mimes(PRIMARY).is_some(),
        "the mirror was replaced by compd's copy, not cleared"
    );
    let mut read = h.extra_clients[b].receive(PRIMARY, TEXT);
    assert_eq!(finish(&mut h, &mut read), b"charlie");
}

#[test]
fn a_client_without_the_keyboard_cannot_take_the_clipboard() {
    let (mut h, b, paster) = two_clients();
    h.focus_keyboard_client(b, &paster);
    h.roundtrip_client(b);
    // The main client has no keyboard focus on any seat.
    h.client.set_selection(PRIMARY, &[TEXT], b"delta");
    h.roundtrip();
    h.roundtrip_client(b);
    assert!(
        h.extra_clients[b].selection_mimes(PRIMARY).is_none(),
        "the copy was refused"
    );
}
