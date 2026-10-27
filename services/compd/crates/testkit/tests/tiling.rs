// SPDX-License-Identifier: MIT OR Apache-2.0
//! Parsed Bus commands and real Super key edges drive the shared executor.

use comp_model::reply::ControlReply;
use comp_model::request::{Request, WindowOp, classify};
use policy::bindings::BindingAction;
use policy_host::geometry;
use serde_json::{Value, json};
use smithay::desktop::Window;
use surfaces::SurfaceId;
use testkit::Harness;
use world::camera::transform::translate::slot;
use world::comp::Placement;

fn fixture() -> (
    Harness,
    SurfaceId,
    u64,
    Window,
    wayland_client::protocol::wl_surface::WlSurface,
    wayland_protocols::xdg::shell::client::xdg_toplevel::XdgToplevel,
) {
    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let id = h
        .comp()
        .registry
        .id_for_handle(&h.handle_of(&surface))
        .unwrap();
    let generation = h.comp().registry.get(id).unwrap().generation();
    let window = h.wire.inner.space.state.elements().next().unwrap().clone();
    h.wire
        .inner
        .space
        .state
        .map_element(window.clone(), (123, 87), false);
    slot::set_expected_size(&window, (640, 480).into());
    (h, id, generation, window, surface, top)
}

/// The production host resolves the owning world; this fixture has one Space.
fn command(h: &mut Harness, window: &Window, name: &str, body: Value) -> Value {
    let op = match classify(name, &body, false) {
        Ok(Request::Window(op)) => op,
        Err((_, body)) => return serde_json::from_str(&body).unwrap(),
        _ => panic!("not a window operation: {name}"),
    };
    let host = &mut h.wire.inner;
    let reply = match op {
        WindowOp::PlaceAs {
            id,
            generation,
            placement,
            output,
        } => geometry::place_as(
            &mut host.comp,
            &mut host.space.state,
            window,
            id,
            generation,
            placement,
            output.as_deref(),
        ),
        WindowOp::TileKey {
            id,
            generation,
            direction,
        } => geometry::tile_key(
            &mut host.comp,
            &mut host.space.state,
            window,
            id,
            generation,
            direction,
        ),
        WindowOp::TilesGet { output, workspace } => {
            geometry::tiles_get(&host.comp, &host.space.state, output.as_deref(), workspace)
        }
        _ => panic!("unexpected operation: {op:?}"),
    };
    reply.wire_json()
}

#[test]
fn bus_placement_slots_saved_geometry_topics_and_query() {
    let (mut h, id, generation, window, _, _) = fixture();
    assert!(h.comp().float_snapshot(id).is_none());
    for tile in world::comp::TileSlot::ALL {
        let body = json!({"id": id.0, "generation": generation, "placement": "tiled", "slot": tile.name()});
        let reply = command(&mut h, &window, "comp.window.place_as", body.clone());
        assert_eq!(reply["placement"], tile.name());
        assert_eq!(reply["maximized"], false);
        assert_eq!(reply["changed"], true);
        assert_eq!(
            reply["float_geometry"],
            json!({"x":123,"y":87,"w":640,"h":480,"output":"o_testkit_0"})
        );
        let events = h.comp().take_placement_events();
        assert_eq!(events.len(), 1);
        let event = events[0].clone().with_event_seq(19);
        assert_eq!(event.topic_suffix(), "window.placement.changed");
        let wire: Value = serde_json::from_str(&event.wire().body).unwrap();
        assert_eq!(
            wire,
            json!({"id":id.0,"generation":generation,"placement":tile.name(),"output":"o_testkit_0","event_seq":19})
        );
        assert_eq!(
            command(&mut h, &window, "comp.window.place_as", body)["changed"],
            false
        );
        assert!(h.comp().take_placement_events().is_empty());
        let query = command(
            &mut h,
            &window,
            "comp.tiles.get",
            json!({"output":"testkit-0"}),
        );
        assert_eq!(query["split"], json!({"x":0.5,"y":0.5}));
        assert_eq!(
            query["slots"][tile.name()],
            json!([{"id":id.0,"generation":generation}])
        );
        let other = command(
            &mut h,
            &window,
            "comp.tiles.get",
            json!({"output":"o_testkit_0", "workspace":2}),
        );
        assert_eq!(other["slots"][tile.name()], json!([]));
    }
    let saved = h.comp().float_geometry(id);
    let reply = command(
        &mut h,
        &window,
        "comp.window.place_as",
        json!({"id":id.0,"generation":generation,"placement":"maximized"}),
    );
    assert_eq!(reply["maximized"], true);
    assert_eq!(h.comp().float_geometry(id), saved);
    let reply = command(
        &mut h,
        &window,
        "comp.window.tile_key",
        json!({"id":id.0,"generation":generation,"direction":"up"}),
    );
    assert_eq!(reply["placement"], "top");
    assert_eq!(h.comp().float_geometry(id), saved);
    let host = &mut h.wire.inner;
    // comp.window.unmaximize uses this same executor, including from a tile.
    geometry::set_maximized(&mut host.comp, &mut host.space.state, id, &window, false);
    assert_eq!(host.comp.placement(id), Placement::Floating);
    assert_eq!(
        host.space.state.element_location(&window),
        Some((123, 87).into())
    );
    assert_eq!(slot::decided_size(&window), Some((640, 480).into()));
    assert_eq!(host.comp.float_geometry(id), saved);
    assert_eq!(
        command(
            &mut h,
            &window,
            "comp.window.place_as",
            json!({"id":id.0,"generation":generation,"placement":"floating"})
        )["changed"],
        false
    );
}

#[test]
fn placement_refusals_leave_geometry_and_float_record_untouched() {
    let (mut h, id, generation, window, surface, top) = fixture();
    // Client-owned min/max hints reach the real protocol cache.
    top.set_min_size(640, 480);
    top.set_max_size(640, 480);
    h.client.commit(&surface);
    h.roundtrip();
    for (placement, error) in [("tiled", "not_resizable"), ("maximized", "not_maximizable")] {
        let mut body = json!({"id":id.0,"generation":generation,"placement":placement});
        if placement == "tiled" {
            body["slot"] = json!("left");
        }
        assert_eq!(
            command(&mut h, &window, "comp.window.place_as", body)["error"],
            error
        );
    }
    for (name, body, expected) in [
        (
            "comp.window.place_as",
            json!({"id":id.0,"generation":generation,"placement":"tiled"}),
            "invalid_argument",
        ),
        (
            "comp.window.place_as",
            json!({"id":id.0,"generation":generation,"placement":"floating","output":"absent"}),
            "unknown_output",
        ),
        (
            "comp.window.tile_key",
            json!({"id":id.0,"generation":generation,"direction":"up"}),
            "not_resizable",
        ),
        (
            "comp.tiles.get",
            json!({"output":"absent"}),
            "unknown_output",
        ),
        (
            "comp.tiles.get",
            json!({"workspace":999}),
            "invalid_argument",
        ),
    ] {
        assert_eq!(command(&mut h, &window, name, body)["error"], expected);
    }
    assert!(h.comp().float_geometry(id).is_none());
    assert!(h.comp().take_placement_events().is_empty());
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((123, 87).into())
    );
    top.set_min_size(0, 0);
    top.set_max_size(800, 600);
    h.client.commit(&surface);
    h.roundtrip();
    assert_eq!(
        command(
            &mut h,
            &window,
            "comp.window.place_as",
            json!({"id":id.0,"generation":generation,"placement":"maximized"})
        )["error"],
        "not_maximizable"
    );
    top.set_max_size(0, 0);
    h.client.commit(&surface);
    h.roundtrip();
    for name in ["comp.window.place_as", "comp.window.tile_key"] {
        let mut body = json!({"id":id.0,"generation":generation+1});
        if name.ends_with("place_as") {
            body["placement"] = json!("floating");
        } else {
            body["direction"] = json!("left");
        }
        assert!(command(&mut h, &window, name, body).get("error").is_some());
    }
    use dispatcher::wire::trait_::wire_trait::WireTrait;
    h.wire.inner.fullscreen_request(window.clone(), true);
    h.service_lifecycle();
    for name in ["comp.window.place_as", "comp.window.tile_key"] {
        let mut body = json!({"id":id.0,"generation":generation});
        if name.ends_with("place_as") {
            body["placement"] = json!("floating");
        } else {
            body["direction"] = json!("left");
        }
        assert_eq!(
            command(&mut h, &window, name, body)["error"],
            "invalid_state"
        );
    }
    assert!(h.comp().float_geometry(id).is_none());
}

#[test]
fn tile_key_crosses_to_mirrored_slot_and_stops_at_outer_edge() {
    use smithay::output::{Output, PhysicalProperties, Subpixel};
    let (mut h, id, generation, window, _, _) = fixture();
    let output = Output::new(
        "east".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "test".into(),
            model: "test".into(),
            serial_number: String::new(),
        },
    );
    output.change_current_state(h.wire.inner.output.current_mode(), None, None, None);
    h.wire.inner.space.state.map_output(&output, (1920, 0));
    let body = |direction| json!({"id":id.0,"generation":generation,"direction":direction});
    assert_eq!(
        command(&mut h, &window, "comp.window.tile_key", body("right"))["placement"],
        "right"
    );
    let saved = h.comp().float_geometry(id);
    let crossed = command(&mut h, &window, "comp.window.tile_key", body("right"));
    assert_eq!(crossed["placement"], "left");
    assert_eq!(crossed["output"], "o_east");
    assert_eq!(h.comp().float_geometry(id), saved);
    command(&mut h, &window, "comp.window.tile_key", body("right"));
    h.comp().take_placement_events();
    assert_eq!(
        command(&mut h, &window, "comp.window.tile_key", body("right"))["changed"],
        false
    );
    assert!(h.comp().take_placement_events().is_empty());
}

#[test]
fn real_super_key_injection_tiles_then_restores_and_toggles_maximize() {
    let (mut h, id, generation, window, surface, _) = fixture();
    h.wire.inner.comp.bindings.ensure("headless");
    h.focus_keyboard(&surface);
    // XKB keycodes: evdev + 8. These are real Left Super chords, never RCtrl.
    for (code, expected) in [
        (105 + 8, "left"),
        (103 + 8, "top_left"),
        (14 + 8, "floating"),
        (104 + 8, "maximized"),
        (104 + 8, "floating"),
    ] {
        h.key(125 + 8, true, false);
        assert!(h.key(code, true, false));
        assert!(h.key(code, false, false));
        h.key(125 + 8, false, false);
        let actions = h.wire.inner.comp.bindings.take_pending();
        assert_eq!(actions.len(), 1);
        let host = &mut h.wire.inner;
        match actions[0] {
            BindingAction::TileFocused(direction) => {
                geometry::tile_key(
                    &mut host.comp,
                    &mut host.space.state,
                    &window,
                    id.0,
                    generation,
                    direction,
                );
            }
            BindingAction::RestoreFocused => {
                geometry::restore_placement(&mut host.comp, &mut host.space.state, id, &window);
            }
            BindingAction::ToggleMaximizeFocused => {
                let enabled = !host.comp.placement(id).maximized();
                geometry::place_as(
                    &mut host.comp,
                    &mut host.space.state,
                    &window,
                    id.0,
                    generation,
                    if enabled {
                        comp_model::request::PlacementKind::Maximized
                    } else {
                        comp_model::request::PlacementKind::Floating
                    },
                    None,
                );
            }
            _ => panic!("unexpected binding"),
        }
        assert_eq!(host.comp.placement(id).name(), expected);
        if expected == "floating" {
            assert_eq!(
                host.space.state.element_location(&window),
                Some((123, 87).into())
            );
            assert_eq!(slot::decided_size(&window), Some((640, 480).into()));
        }
    }
}

#[test]
fn restore_exits_fullscreen_before_untiling_and_unmaximize_keeps_the_float() {
    use dispatcher::wire::trait_::wire_trait::WireTrait;
    let (mut h, id, generation, window, _, _) = fixture();
    command(
        &mut h,
        &window,
        "comp.window.place_as",
        json!({"id":id.0,"generation":generation,"placement":"tiled","slot":"bottom_right"}),
    );
    let saved = h.comp().float_geometry(id);
    h.wire.inner.fullscreen_request(window.clone(), true);
    h.service_lifecycle();
    let host = &mut h.wire.inner;
    assert_eq!(
        geometry::restore_placement(&mut host.comp, &mut host.space.state, id, &window),
        geometry::RestoreChange::Fullscreen
    );
    assert_eq!(host.comp.placement(id).name(), "bottom_right");
    host.fullscreen_request(window.clone(), false);
    h.service_lifecycle();
    h.roundtrip();
    let host = &mut h.wire.inner;
    assert!(matches!(
        geometry::restore_placement(&mut host.comp, &mut host.space.state, id, &window),
        geometry::RestoreChange::Placement(_)
    ));
    assert_eq!(host.comp.placement(id), Placement::Floating);
    assert_eq!(host.comp.float_geometry(id), saved);
    assert_eq!(
        host.space.state.element_location(&window),
        Some((123, 87).into())
    );
    geometry::set_maximized(&mut host.comp, &mut host.space.state, id, &window, true);
    geometry::set_maximized(&mut host.comp, &mut host.space.state, id, &window, false);
    assert!(matches!(
        geometry::restore_placement(&mut host.comp, &mut host.space.state, id, &window),
        geometry::RestoreChange::Placement(geometry::GeometryChange { windows: false, .. })
    ));
    assert_eq!(host.comp.float_geometry(id), saved);
    assert_eq!(slot::decided_size(&window), Some((640, 480).into()));
}
