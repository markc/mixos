// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Real interactive move/resize grabs: a client
//! names the button serial it was given in `xdg_toplevel.move` / `.resize`,
//! the grab starts on the primary seat and reports the pointer's travel;
//! the agent seat and fullscreen windows are refused.
//!
//! Grabs report cumulative deltas into `CompState::interactive`. The host and
//! these fixtures apply them through the same renderer-free geometry executor.

use policy_host::geometry;
use smithay::desktop::Window;
use smithay::input::pointer::{ButtonEvent, MotionEvent};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::utils::{Point, SERIAL_COUNTER};
use surfaces::SurfaceId;
use testkit::Harness;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
use world::camera::transform::translate::slot;
use world::comp::Placement;

const BTN_LEFT: u32 = 0x110;
/// `TestClient::seat` indices: the primary seat is advertised first.
const PRIMARY: usize = 0;
const AGENT: usize = 1;

/// Move the server's primary pointer to `location` (grab-routed while a grab
/// holds the pointer) and let the drain run.
fn motion(h: &mut Harness, location: (f64, f64)) {
    let pointer = h
        .wire
        .state
        .seat
        .seat
        .get_pointer()
        .expect("primary pointer");
    let dispatch = &mut h.wire.state;
    pointer.motion(
        dispatch,
        None,
        &MotionEvent {
            location: Point::from(location),
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    pointer.frame(dispatch);
    h.roundtrip();
}

/// Let the button go on the server's primary pointer. Not `Harness::release`:
/// the grab clears client focus, so the client sees no release to assert on.
fn let_go(h: &mut Harness) {
    let pointer = h
        .wire
        .state
        .seat
        .seat
        .get_pointer()
        .expect("primary pointer");
    let dispatch = &mut h.wire.state;
    pointer.button(
        dispatch,
        &ButtonEvent {
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
            button: BTN_LEFT,
            state: smithay::backend::input::ButtonState::Released,
        },
    );
    pointer.frame(dispatch);
    h.roundtrip();
}

/// Commit `state` on the server toplevel of `surface` and have the client
/// ack and commit it.
fn commit_state(h: &mut Harness, surface: &WlSurface, state: State) {
    let handle = h.handle_of(surface);
    let toplevel = h
        .wire
        .inner
        .space
        .state
        .elements()
        .find(|window| {
            dispatcher::wire::trait_::surface_event::SurfaceHandle::of_window(window).as_ref()
                == Some(&handle)
        })
        .and_then(|window| window.toplevel().cloned())
        .expect("the window is in the Space");
    toplevel.with_pending_state(|pending| {
        pending.states.set(state);
    });
    toplevel.send_configure();
    h.roundtrip();
    h.client.commit(surface);
    h.roundtrip();
}

#[test]
fn a_move_on_the_primary_seat_grabs_and_reports_the_travel() {
    let mut h = Harness::new();
    let (surface, _xdg, toplevel) = h.mapped_toplevel(64, 48);
    let serial = h.press(&surface, BTN_LEFT);
    toplevel._move(h.client.seat(PRIMARY), serial);
    h.roundtrip();
    let grab = h.comp().interactive.expect("the move started a grab");
    assert_eq!((grab.edges, grab.updated, grab.ended), (0, false, false));
    // The harness pressed at (1, 1).
    motion(&mut h, (13.0, 4.0));
    let grab = h.comp().interactive.expect("still grabbing");
    assert_eq!((grab.delta, grab.updated), ((12.0, 3.0), true));
    let_go(&mut h);
    assert!(
        h.comp()
            .interactive
            .expect("the record waits for the host")
            .ended
    );
}

#[test]
fn a_resize_carries_its_edges() {
    let mut h = Harness::new();
    let (surface, _xdg, toplevel) = h.mapped_toplevel(64, 48);
    let serial = h.press(&surface, BTN_LEFT);
    toplevel.resize(h.client.seat(PRIMARY), serial, ResizeEdge::BottomRight);
    h.roundtrip();
    assert_eq!(h.comp().interactive.map(|grab| grab.edges), Some(10));
    motion(&mut h, (21.0, 31.0));
    assert_eq!(
        h.comp().interactive.map(|grab| grab.delta),
        Some((20.0, 30.0))
    );
}

#[test]
fn the_agent_seat_cannot_move_a_window() {
    let mut h = Harness::new();
    let (surface, _xdg, toplevel) = h.mapped_toplevel(64, 48);
    // A valid primary-seat serial: only the seat differs from the passing case.
    let serial = h.press(&surface, BTN_LEFT);
    toplevel._move(h.client.seat(AGENT), serial);
    h.roundtrip();
    assert!(h.comp().interactive.is_none(), "the agent seat is refused");
    toplevel.resize(h.client.seat(AGENT), serial, ResizeEdge::Right);
    h.roundtrip();
    assert!(h.comp().interactive.is_none());
}

#[test]
fn a_maximised_window_can_start_a_move() {
    let mut h = Harness::new();
    let (surface, _xdg, toplevel) = h.mapped_toplevel(64, 48);
    commit_state(&mut h, &surface, State::Maximized);
    let serial = h.press(&surface, BTN_LEFT);
    toplevel._move(h.client.seat(PRIMARY), serial);
    h.roundtrip();
    assert!(
        h.comp().interactive.is_some(),
        "a committed-maximised window can be dragged away"
    );
}

#[test]
fn a_stale_serial_starts_nothing() {
    let mut h = Harness::new();
    let (surface, _xdg, toplevel) = h.mapped_toplevel(64, 48);
    let serial = h.press(&surface, BTN_LEFT);
    toplevel._move(h.client.seat(PRIMARY), serial.wrapping_add(1000));
    h.roundtrip();
    assert!(
        h.comp().interactive.is_none(),
        "the serial must name the held button"
    );
}

fn window(h: &Harness, surface: &WlSurface) -> (SurfaceId, Window) {
    let handle = h.handle_of(surface);
    let id = h.comp().registry.id_for_handle(&handle).unwrap();
    let window = h
        .wire
        .inner
        .space
        .state
        .elements()
        .find(|window| {
            dispatcher::wire::trait_::surface_event::SurfaceHandle::of_window(window).as_ref()
                == Some(&handle)
        })
        .unwrap()
        .clone();
    (id, window)
}

/// Press at a real frame coordinate, with client focus or compositor chrome.
fn press_at(h: &mut Harness, window: &Window, at: (f64, f64), chrome: bool) -> u32 {
    let pointer = h.wire.state.seat.seat.get_pointer().unwrap();
    let focus = if chrome {
        None
    } else {
        Some((
            window.toplevel().unwrap().wl_surface().clone(),
            (0.0, 0.0).into(),
        ))
    };
    pointer.motion(
        &mut h.wire.state,
        focus,
        &MotionEvent {
            location: at.into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    let serial = SERIAL_COUNTER.next_serial();
    pointer.button(
        &mut h.wire.state,
        &ButtonEvent {
            serial,
            time: 0,
            button: BTN_LEFT,
            state: smithay::backend::input::ButtonState::Pressed,
        },
    );
    pointer.frame(&mut h.wire.state);
    h.roundtrip();
    serial.into()
}

fn apply(h: &mut Harness, window: &Window) -> bool {
    let host = &mut h.wire.inner;
    let mut grab = host.comp.interactive.take().unwrap();
    let changed =
        geometry::apply_interactive(&mut host.comp, &mut host.space.state, window, &mut grab);
    if !grab.ended {
        host.comp.interactive = Some(grab);
    }
    changed
}

fn drag_maximised(chrome: bool) {
    let mut h = Harness::new();
    h.wire.inner.output.change_current_state(
        Some(smithay::output::Mode {
            size: (3840, 2160).into(),
            refresh: 60_000,
        }),
        None,
        None,
        None,
    );
    let (surface, _, top) = h.mapped_toplevel(1200, 800);
    let (id, window) = window(&h, &surface);
    if chrome {
        decor::window::install(decor::ChromeTheme::from_source(
            decor::ChromeStyle::Win11,
            None,
        ));
        window.toplevel().unwrap().with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        window.toplevel().unwrap().send_configure();
        h.roundtrip();
        h.client.commit(&surface);
        h.roundtrip();
        assert!(decor::window::extents(&window).is_some());
    }
    {
        let host = &mut h.wire.inner;
        geometry::set_maximized(&mut host.comp, &mut host.space.state, id, &window, true);
    }
    h.roundtrip();
    let saved = h.comp().float_geometry(id).unwrap();
    assert!(decor::window::constrained(&window));
    let old_origin = h.wire.inner.space.state.element_location(&window).unwrap();
    let old_size = slot::decided_size(&window).unwrap();
    let at = (2560.0, 10.0);
    let serial = press_at(&mut h, &window, at, chrome);
    if chrome {
        assert!(dispatcher::wayland::grab::interactive::start_chrome(
            &mut h.wire.state,
            &window,
            0
        ));
    } else {
        top._move(h.client.seat(PRIMARY), serial);
    }
    h.roundtrip();
    // A click without travel does not unmaximise. Host captures the frame anchor.
    assert!(!apply(&mut h, &window));
    assert!(h.comp().placement(id).maximized());
    motion(&mut h, (2560.0, 10.0));
    apply(&mut h, &window);
    assert!(h.comp().placement(id).maximized());
    h.client.state.hold_xdg_configures = true;
    // Coalescing retains the original pointer and only the final cumulative delta.
    motion(&mut h, (2570.0, 20.0));
    motion(&mut h, (2608.0, 50.0));
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some(old_origin)
    );
    assert_eq!(slot::decided_size(&window), Some(old_size));
    assert!(apply(&mut h, &window));
    assert_eq!(h.comp().placement(id), Placement::Floating);
    assert_eq!(slot::decided_size(&window), Some(saved.size));
    assert!(!decor::window::constrained(&window));
    let e = decor::window::extents(&window);
    let (left, top_extent, right, bottom) = e.map_or((0.0, 0.0, 0.0, 0.0), |e| {
        (
            f64::from(e.left),
            f64::from(e.top),
            f64::from(e.right),
            f64::from(e.bottom),
        )
    });
    let expected = Point::from((
        (2608.0 - (2.0 / 3.0) * (1200.0 + left + right) + left).round() as i32,
        (50.0 - (10.0 / 2160.0) * (800.0 + top_extent + bottom) + top_extent).round() as i32,
    ));
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some(expected)
    );
    if chrome {
        let relative = (2608.0 - f64::from(expected.x), 50.0 - f64::from(expected.y));
        assert!(
            relative.1 < 0.0 && relative.1 >= -top_extent,
            "pointer stays on restored titlebar"
        );
        assert!(decor::window::hit(&window, relative, saved.size).is_some());
    } else {
        assert_eq!(
            2608 - expected.x,
            800,
            "the 3840-to-1200 example anchors exactly"
        );
    }
    h.roundtrip();
    assert_eq!(
        slot::decided_size(&window),
        Some(saved.size),
        "ACK delay cannot expose a displaced maximised frame"
    );
    motion(&mut h, (2620.0, 57.0));
    assert!(apply(&mut h, &window));
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some(expected + Point::from((12, 7)))
    );
    assert_eq!(h.comp().float_geometry(id), Some(saved));
    let_go(&mut h);
    apply(&mut h, &window);
    assert!(h.comp().interactive.is_none());
}

#[test]
fn client_move_restores_maximised_size_and_proportional_anchor_in_one_pass() {
    drag_maximised(false);
}

#[test]
fn chrome_drag_restores_maximised_size_and_keeps_pointer_on_titlebar() {
    drag_maximised(true);
}

#[test]
fn fullscreen_refuses_client_and_chrome_moves() {
    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let (_, window) = window(&h, &surface);
    commit_state(&mut h, &surface, State::Fullscreen);
    let serial = h.press(&surface, BTN_LEFT);
    top._move(h.client.seat(PRIMARY), serial);
    h.roundtrip();
    assert!(h.comp().interactive.is_none());
    assert!(!dispatcher::wayland::grab::interactive::start_chrome(
        &mut h.wire.state,
        &window,
        0
    ));
}

#[test]
fn a_maximised_window_still_refuses_client_resize() {
    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    commit_state(&mut h, &surface, State::Maximized);
    let serial = h.press(&surface, BTN_LEFT);
    top.resize(h.client.seat(PRIMARY), serial, ResizeEdge::BottomRight);
    h.roundtrip();
    assert!(h.comp().interactive.is_none());
}

#[test]
fn mapped_maximised_without_float_geometry_gets_a_pointer_centred_default() {
    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(1920, 1080);
    let (id, window) = window(&h, &surface);
    h.wire.inner.comp.set_placement(
        id,
        &window,
        Placement::Maximized {
            output: "testkit-0".into(),
        },
    );
    assert!(h.comp().float_geometry(id).is_none());
    protocols::window::shell::set_maximized(&window, true);
    protocols::window::shell::send(&window);
    h.roundtrip();
    let serial = press_at(&mut h, &window, (1280.0, 10.0), false);
    top._move(h.client.seat(PRIMARY), serial);
    h.roundtrip();
    // Begin and motion may arrive before the host ever observes the grab.
    motion(&mut h, (1300.0, 30.0));
    assert!(apply(&mut h, &window));
    assert_eq!(slot::decided_size(&window), Some((1280, 720).into()));
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((660, -330).into())
    );
    assert_eq!(h.comp().placement(id), Placement::Floating);
}

#[test]
fn a_tiled_move_restores_pre_tile_geometry_even_if_released_in_the_first_pass() {
    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    {
        let host = &mut h.wire.inner;
        geometry::set_placement(
            &mut host.comp,
            &mut host.space.state,
            id,
            &window,
            Placement::Tiled {
                slot: world::comp::TileSlot::Right,
                output: "testkit-0".into(),
            },
        );
    }
    let serial = press_at(&mut h, &window, (1600.0, 10.0), false);
    top._move(h.client.seat(PRIMARY), serial);
    h.roundtrip();
    motion(&mut h, (1620.0, 40.0));
    let_go(&mut h);
    assert!(apply(&mut h, &window));
    assert_eq!(h.comp().placement(id), Placement::Floating);
    assert_eq!(slot::decided_size(&window), Some((640, 480).into()));
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((1193, 36).into())
    );
    assert!(h.comp().interactive.is_none());
}

#[test]
fn external_unmaximise_before_first_motion_rebases_the_host_move_start() {
    for chrome in [false, true] {
        let mut h = Harness::new();
        let (surface, _, top) = h.mapped_toplevel(640, 480);
        let (id, window) = window(&h, &surface);
        h.wire
            .inner
            .space
            .state
            .map_element(window.clone(), (80, 60), false);
        {
            let host = &mut h.wire.inner;
            geometry::set_maximized(&mut host.comp, &mut host.space.state, id, &window, true);
        }
        let serial = press_at(&mut h, &window, (1280.0, 10.0), chrome);
        if chrome {
            assert!(dispatcher::wayland::grab::interactive::start_chrome(
                &mut h.wire.state,
                &window,
                0,
            ));
        } else {
            top._move(h.client.seat(PRIMARY), serial);
        }
        h.roundtrip();
        assert!(!apply(&mut h, &window));
        assert_eq!(
            h.comp().interactive.unwrap().start,
            Some(((0, 0).into(), (1920, 1080).into()))
        );

        // Another request restores the window after the host captured M but
        // before the pointer has travelled. The next pass must start from A.
        {
            let host = &mut h.wire.inner;
            assert!(
                geometry::set_maximized(&mut host.comp, &mut host.space.state, id, &window, false)
                    .windows
            );
        }
        assert_eq!(h.comp().placement(id), Placement::Floating);
        assert_eq!(
            h.wire.inner.space.state.element_location(&window),
            Some((80, 60).into())
        );
        motion(&mut h, (1300.0, 40.0));
        assert!(apply(&mut h, &window));
        assert_eq!(
            h.wire.inner.space.state.element_location(&window),
            Some((100, 90).into())
        );
        assert_eq!(slot::decided_size(&window), Some((640, 480).into()));
        let grab = h.comp().interactive.unwrap();
        assert_eq!(grab.pointer_start, (1280.0, 10.0).into());
        assert_eq!(grab.delta, (20.0, 30.0));
        assert_eq!(grab.start, Some(((80, 60).into(), (640, 480).into())));

        motion(&mut h, (1307.0, 45.0));
        assert!(apply(&mut h, &window));
        assert_eq!(
            h.wire.inner.space.state.element_location(&window),
            Some((107, 95).into())
        );
        let_go(&mut h);
        apply(&mut h, &window);
        assert!(h.comp().interactive.is_none());
    }
}
