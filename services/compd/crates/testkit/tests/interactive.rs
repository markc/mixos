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
    let shift = h
        .wire
        .state
        .seat
        .seat
        .get_keyboard()
        .unwrap()
        .modifier_state()
        .shift;
    let host = &mut h.wire.inner;
    let mut grab = host.comp.interactive.take().unwrap();
    let pointer = grab.pointer_start + Point::from(grab.delta);
    let changed = geometry::apply_interactive_at(
        &mut host.comp,
        &mut host.space.state,
        window,
        &mut grab,
        pointer,
        shift,
    );
    if !grab.ended {
        host.comp.interactive = Some(grab);
    }
    changed
}

fn begin_move(h: &mut Harness, window: &Window) {
    press_at(h, window, (300.0, 200.0), true);
    assert!(dispatcher::wayland::grab::interactive::start_chrome(
        &mut h.wire.state,
        window,
        0
    ));
    h.roundtrip();
    apply(h, window);
}

#[test]
fn client_move_released_before_first_host_pass_commits_and_saves_start_frame() {
    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    h.wire
        .inner
        .space
        .state
        .map_element(window.clone(), (100, 80), false);
    let serial = press_at(&mut h, &window, (300.0, 200.0), false);
    top._move(h.client.seat(PRIMARY), serial);
    h.roundtrip();
    motion(&mut h, (0.0, 540.0));
    let_go(&mut h);
    apply(&mut h, &window);
    assert_eq!(
        h.comp().placement(id),
        Placement::Tiled {
            slot: world::comp::TileSlot::Left,
            output: "testkit-0".into()
        }
    );
    let saved = h.comp().float_geometry(id).unwrap();
    assert_eq!(
        (saved.location, saved.size),
        ((100, 80).into(), (640, 480).into())
    );
    assert!(h.comp().interactive.is_none());
}

fn preview(
    h: &Harness,
) -> Vec<smithay::backend::renderer::element::solid::SolidColorRenderElement> {
    world::comp::tiling::preview(h.comp(), &h.wire.inner.space.state, "testkit-0")
}

#[test]
fn every_drag_zone_commits_its_placement_frame_and_protocol_states() {
    use world::comp::{TileSlot, TileSplit};
    for (point, tile) in [
        ((0.0, 540.0), Some(TileSlot::Left)),
        ((1919.0, 540.0), Some(TileSlot::Right)),
        ((0.0, 0.0), Some(TileSlot::TopLeft)),
        ((1919.0, 0.0), Some(TileSlot::TopRight)),
        ((0.0, 1079.0), Some(TileSlot::BottomLeft)),
        ((1919.0, 1079.0), Some(TileSlot::BottomRight)),
        ((960.0, 0.0), None),
    ] {
        let mut h = Harness::new();
        let (surface, _, _) = h.mapped_toplevel(640, 480);
        let (id, window) = window(&h, &surface);
        h.wire
            .inner
            .space
            .state
            .map_element(window.clone(), (100, 100), false);
        begin_move(&mut h, &window);
        motion(&mut h, point);
        apply(&mut h, &window);
        assert_eq!(preview(&h).len(), 5);
        let area = smithay::utils::Rectangle::new((0, 0).into(), (1920, 1080).into());
        let expected = tile.map_or(area, |tile| tile.rectangle(area, TileSplit::default()));
        assert_eq!(
            h.comp()
                .drag
                .as_ref()
                .unwrap()
                .armed
                .as_ref()
                .unwrap()
                .rectangle,
            expected
        );
        let_go(&mut h);
        apply(&mut h, &window);
        assert_eq!(
            h.comp().placement(id),
            tile.map_or(
                Placement::Maximized {
                    output: "testkit-0".into()
                },
                |slot| Placement::Tiled {
                    slot,
                    output: "testkit-0".into()
                }
            )
        );
        assert_eq!(
            h.wire.inner.space.state.element_location(&window),
            Some(expected.loc)
        );
        assert_eq!(slot::decided_size(&window), Some(expected.size));
        assert!(decor::window::constrained(&window));
        assert!(preview(&h).is_empty());
        let saved = h.comp().float_geometry(id).unwrap();
        assert_eq!(
            (saved.location, saved.size),
            ((100, 100).into(), (640, 480).into()),
            "save the start, not release frame"
        );
        h.roundtrip();
        for edge in [
            State::TiledLeft,
            State::TiledRight,
            State::TiledTop,
            State::TiledBottom,
        ] {
            assert_eq!(
                window
                    .toplevel()
                    .unwrap()
                    .with_pending_state(|state| state.states.contains(edge)),
                tile.is_some()
            );
            assert_eq!(
                h.client
                    .state
                    .toplevel_configures
                    .last()
                    .unwrap()
                    .states
                    .contains(&(edge as u32)),
                tile.is_some(),
                "tiled state reaches the client"
            );
        }
        assert_eq!(
            window
                .toplevel()
                .unwrap()
                .with_pending_state(|state| state.states.contains(State::Maximized)),
            tile.is_none()
        );
        {
            let host = &mut h.wire.inner;
            geometry::set_placement(
                &mut host.comp,
                &mut host.space.state,
                id,
                &window,
                Placement::Floating,
            );
        }
        for edge in [
            State::TiledLeft,
            State::TiledRight,
            State::TiledTop,
            State::TiledBottom,
        ] {
            assert!(
                !window
                    .toplevel()
                    .unwrap()
                    .with_pending_state(|state| state.states.contains(edge))
            );
        }
        h.roundtrip();
        for edge in [
            State::TiledLeft,
            State::TiledRight,
            State::TiledTop,
            State::TiledBottom,
        ] {
            assert!(
                !h.client
                    .state
                    .toplevel_configures
                    .last()
                    .unwrap()
                    .states
                    .contains(&(edge as u32))
            );
        }
    }
}

#[test]
fn preview_changes_rectangle_hides_and_reuses_ids() {
    use smithay::backend::renderer::element::Element;
    let mut h = Harness::new();
    let (surface, _, _) = h.mapped_toplevel(640, 480);
    let (_, window) = window(&h, &surface);
    begin_move(&mut h, &window);
    motion(&mut h, (0.0, 540.0));
    apply(&mut h, &window);
    let left = preview(&h);
    assert_eq!(
        left[4].geometry(1.0.into()),
        smithay::utils::Rectangle::new((0, 0).into(), (960, 1080).into())
    );
    assert_eq!(left[4].alpha(), world::comp::tiling::PREVIEW_ALPHA);
    motion(&mut h, (1919.0, 0.0));
    apply(&mut h, &window);
    let right = preview(&h);
    assert_eq!(
        right[4].geometry(1.0.into()),
        smithay::utils::Rectangle::new((960, 0).into(), (960, 540).into())
    );
    for (a, b) in left.iter().zip(&right) {
        assert_eq!(a.id(), b.id());
    }
    h.key(50, true, false);
    apply(&mut h, &window); // no pointer motion: every host pass sees modifiers
    assert!(preview(&h).is_empty());
    h.key(50, false, false);
    apply(&mut h, &window);
    assert_eq!(preview(&h).len(), 5);
    motion(&mut h, (960.0, 1079.0));
    apply(&mut h, &window);
    assert!(preview(&h).is_empty());
    let_go(&mut h);
    apply(&mut h, &window);
    assert!(preview(&h).is_empty());
    assert!(h.comp().drag.is_none());
}

#[test]
fn panel_coordinates_count_and_shift_fixed_size_and_max_hints_skip_zones() {
    for scenario in ["panel", "shift", "fixed", "max", "bottom"] {
        let mut h = Harness::new();
        let (surface, _, top) = h.mapped_toplevel(640, 480);
        let (id, window) = window(&h, &surface);
        if scenario == "panel" {
            h.wire.inner.comp.reserved.insert(
                "testkit-0".into(),
                world::comp::usable::Reserved {
                    left: 60,
                    ..Default::default()
                },
            );
        } else if scenario == "fixed" {
            top.set_min_size(640, 480);
            top.set_max_size(640, 480);
            h.client.commit(&surface);
            h.roundtrip();
        } else if scenario == "max" {
            top.set_max_size(800, 600);
            h.client.commit(&surface);
            h.roundtrip();
        } else if scenario == "shift" {
            h.key(50, true, false);
        }
        begin_move(&mut h, &window);
        let point = match scenario {
            "max" => (960.0, 0.0),
            "bottom" => (960.0, 1079.0),
            _ => (0.0, 540.0),
        };
        motion(&mut h, point);
        apply(&mut h, &window);
        let_go(&mut h);
        apply(&mut h, &window);
        if scenario == "panel" {
            assert_eq!(
                h.comp().placement(id),
                Placement::Tiled {
                    slot: world::comp::TileSlot::Left,
                    output: "testkit-0".into()
                }
            );
            assert_eq!(
                h.wire.inner.space.state.element_location(&window),
                Some((60, 0).into())
            );
            assert_eq!(slot::decided_size(&window), Some((930, 1080).into()));
        } else {
            assert_eq!(h.comp().placement(id), Placement::Floating);
        }
    }
}

#[test]
fn shared_edges_do_not_arm_but_outer_edges_on_both_outputs_do() {
    use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
    let mut h = Harness::new();
    let secondary = Output::new(
        "testkit-1".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "testkit".into(),
            model: "headless".into(),
            serial_number: "testkit-1".into(),
        },
    );
    secondary.change_current_state(
        Some(Mode {
            size: (1280, 720).into(),
            refresh: 60000,
        }),
        None,
        None,
        None,
    );
    h.wire.inner.space.state.map_output(&secondary, (1920, 0));
    let (surface, _, _) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    begin_move(&mut h, &window);
    for point in [
        (1919.0, 400.0),
        (1920.0, 400.0),
        (1919.0, 0.0),
        (1920.0, 0.0),
    ] {
        motion(&mut h, point);
        apply(&mut h, &window);
        assert!(h.comp().drag.as_ref().unwrap().armed.is_none());
    }
    motion(&mut h, (0.0, 540.0));
    apply(&mut h, &window);
    assert!(h.comp().drag.as_ref().unwrap().armed.is_some());
    motion(&mut h, (3199.0, 400.0));
    apply(&mut h, &window);
    let_go(&mut h);
    apply(&mut h, &window);
    assert_eq!(
        h.comp().placement(id),
        Placement::Tiled {
            slot: world::comp::TileSlot::Right,
            output: "testkit-1".into()
        }
    );
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((2560, 0).into())
    );
    assert_eq!(slot::decided_size(&window), Some((640, 720).into()));
}

#[test]
fn escape_restores_start_geometry_placement_and_saved_float_without_client_keys() {
    for start in [
        Placement::Floating,
        Placement::Maximized {
            output: "testkit-0".into(),
        },
        Placement::Tiled {
            slot: world::comp::TileSlot::Left,
            output: "testkit-0".into(),
        },
    ] {
        let mut h = Harness::new();
        let (surface, _, _) = h.mapped_toplevel(640, 480);
        let (id, window) = window(&h, &surface);
        h.client.bind_keyboards();
        h.focus_keyboard(&surface);
        {
            let host = &mut h.wire.inner;
            host.space
                .state
                .map_element(window.clone(), (100, 80), false);
            geometry::set_placement(
                &mut host.comp,
                &mut host.space.state,
                id,
                &window,
                start.clone(),
            );
        }
        let origin = h.wire.inner.space.state.element_location(&window);
        let size = slot::size_of(&window).unwrap_or(window.geometry().size);
        let saved = h.comp().float_geometry(id);
        begin_move(&mut h, &window);
        motion(&mut h, (0.0, 540.0));
        apply(&mut h, &window);
        assert_eq!(h.comp().placement(id), Placement::Floating);
        assert!(!preview(&h).is_empty());
        assert!(h.key(9, true, false), "Escape press consumed");
        assert!(
            !h.wire
                .state
                .seat
                .seat
                .get_pointer()
                .unwrap()
                .with_grab(
                    |_, grab| grab.is::<dispatcher::wayland::grab::interactive::InteractiveGrab>()
                )
                .unwrap_or(false)
        );
        assert!(h.comp().interactive.unwrap().cancelled);
        apply(&mut h, &window);
        assert!(h.comp().interactive.is_none());
        assert_eq!(h.comp().placement(id), start);
        assert_eq!(h.wire.inner.space.state.element_location(&window), origin);
        assert_eq!(slot::size_of(&window), Some(size));
        assert_eq!(h.comp().float_geometry(id), saved);
        assert!(preview(&h).is_empty());
        assert!(
            h.key(9, false, false),
            "Escape release consumed after grab end"
        );
        assert!(h.client.state.keys.is_empty());
        let_go(&mut h);
        assert!(h.comp().interactive.is_none());
    }
}

#[test]
fn a_tiled_start_keeps_its_original_float_when_released_in_another_tile() {
    let mut h = Harness::new();
    let (surface, _, _) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    {
        let host = &mut h.wire.inner;
        geometry::set_placement(
            &mut host.comp,
            &mut host.space.state,
            id,
            &window,
            Placement::Tiled {
                slot: world::comp::TileSlot::Left,
                output: "testkit-0".into(),
            },
        );
    }
    let saved = h.comp().float_geometry(id);
    begin_move(&mut h, &window);
    motion(&mut h, (1919.0, 1079.0));
    let_go(&mut h); // coalesced motion + release, before a host pass
    apply(&mut h, &window);
    assert_eq!(
        h.comp().placement(id),
        Placement::Tiled {
            slot: world::comp::TileSlot::BottomRight,
            output: "testkit-0".into()
        }
    );
    assert_eq!(h.comp().float_geometry(id), saved);
}

#[test]
fn release_on_an_engaged_corner_still_reaches_the_move_grab_and_commits() {
    let mut h = Harness::new();
    let (surface, _, _) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    assert!(!h.wire.inner.comp.corners.press(BTN_LEFT, Vec::new(), false));
    begin_move(&mut h, &window);
    motion(&mut h, (1.0, 1.0));
    apply(&mut h, &window);
    let corners = &mut h.wire.inner.comp.corners;
    let mut config = corners.config();
    config.dwell_ms = 0;
    corners.set_config(config);
    corners.sample("testkit-0", (1920.0, 1080.0), (1.0, 1.0), (0.0, 0.0), false);
    std::thread::sleep(std::time::Duration::from_millis(2));
    corners.tick();
    assert!(corners.engaged().is_some());
    assert!(
        !corners.release(BTN_LEFT),
        "the titlebar press never belonged to the corner"
    );
    let_go(&mut h);
    apply(&mut h, &window);
    assert!(h.comp().interactive.is_none());
    assert_eq!(
        h.comp().placement(id),
        Placement::Tiled {
            slot: world::comp::TileSlot::TopLeft,
            output: "testkit-0".into()
        }
    );
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
