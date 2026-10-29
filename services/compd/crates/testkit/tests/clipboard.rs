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
fn two_clients() -> (
    Harness,
    usize,
    wayland_client::protocol::wl_surface::WlSurface,
) {
    let mut h = Harness::new();
    h.client.bind_data_devices();
    h.roundtrip();
    assert_eq!(
        h.client.seat_count(),
        2,
        "the primary seat and the agent seat"
    );
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
    // And it survives that client's source going away, like any copy, once the
    // capture has the bytes (as for a primary-seat copy: a source destroyed while
    // its capture is still in flight leaves nothing to persist).
    for _ in 0..400 {
        if !h.wire.state.clipboard.capture.is_empty() {
            break;
        }
        h.pump();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        !h.wire.state.clipboard.capture.is_empty(),
        "the capture finished"
    );
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

#[test]
fn first_middle_click_after_focus_receives_primary_before_button() {
    let mut h = Harness::new();
    h.client.bind_primary_devices();
    let (target, ..) = h.mapped_toplevel(64, 48);
    let owner = h.add_client();
    h.extra_clients[owner].bind_primary_devices();
    let source_surface = h.extra_clients[owner].create_surface();
    h.extra_clients[owner].commit(&source_surface);
    h.roundtrip_client(owner);
    h.focus_keyboard_client(owner, &source_surface);
    let _source = h.extra_clients[owner].set_primary_selection(PRIMARY, b"first-click-primary");
    h.roundtrip_client(owner);
    h.roundtrip();
    assert!(
        h.client
            .state
            .clipboard
            .primary_selections
            .get(&PRIMARY)
            .is_none_or(|offer| offer.is_none()),
        "the unfocused target must not already hold a PRIMARY offer"
    );

    // In particular, do NOT use focus_keyboard(): its roundtrip would deliver
    // a deferred offer before the click and make the broken ordering pass.
    let first = h.focus_and_press(&target, 0x112);
    let (serial, seat, read) = h
        .client
        .state
        .clipboard
        .primary_pastes
        .pop()
        .expect("the first middle press was observed");
    assert_eq!((serial, seat), (first, PRIMARY));
    let mut read = read.expect("PRIMARY must be offered at the FIRST middle-button callback");
    assert_eq!(finish(&mut h, &mut read), b"first-click-primary");
    h.release(&target, 0x112);

    // The already-focused control must still paste once, with the same bytes.
    let second = h.press(&target, 0x112);
    let (serial, seat, read) = h.client.state.clipboard.primary_pastes.pop().unwrap();
    assert_eq!((serial, seat), (second, PRIMARY));
    let mut read = read.expect("focused middle press retains PRIMARY");
    assert_eq!(finish(&mut h, &mut read), b"first-click-primary");
    h.release(&target, 0x112);

    h.clear_keyboard_focus();
    // Losing focus revokes selection authority; it need not send selection(nil)
    // to the former client. An unfocused replacement must not displace the owner.
    let _refused = h
        .client
        .set_primary_selection(PRIMARY, b"unfocused-replacement");
    h.roundtrip();
    h.focus_and_press(&target, 0x112);
    let (_, _, read) = h.client.state.clipboard.primary_pastes.pop().unwrap();
    let mut read = read.expect("refocused client receives the original PRIMARY");
    assert_eq!(finish(&mut h, &mut read), b"first-click-primary");
    h.release(&target, 0x112);
}

#[test]
fn agent_keyboard_focus_does_not_redirect_primary_selection() {
    let mut h = Harness::new();
    h.client.bind_primary_devices();
    let (target, ..) = h.mapped_toplevel(64, 48);
    let owner = h.add_client();
    h.extra_clients[owner].bind_primary_devices();
    let source_surface = h.extra_clients[owner].create_surface();
    h.extra_clients[owner].commit(&source_surface);
    h.roundtrip_client(owner);
    h.focus_keyboard_client(owner, &source_surface);
    let _source = h.extra_clients[owner].set_primary_selection(PRIMARY, b"physical-seat-only");
    h.roundtrip_client(owner);
    h.focus_agent_keyboard(&target);
    h.roundtrip_client(owner);
    assert!(h.extra_clients[owner].state.clipboard.primary_selections[&PRIMARY].is_some());
    assert!(
        h.client
            .state
            .clipboard
            .primary_selections
            .get(&PRIMARY)
            .is_none_or(|offer| offer.is_none()),
        "agent focus cannot move physical PRIMARY"
    );
}
