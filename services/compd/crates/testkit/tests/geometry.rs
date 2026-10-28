// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Production geometry execution and real xdg ACK/commit semantics. This fixture
//! has no renderer, scene settings worker or GPU: fitted coordinates are protocol
//! geometry evidence, not native pixels or a settings-to-frame timing measure.

use policy_host::geometry::{self, GeometryChange};
use protocols::window::shell;
use smithay::desktop::Window;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::utils::{Size, Transform};
use surfaces::SurfaceId;
use testkit::{Harness, client::protocol_id};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols::xdg::shell::client::{
    xdg_surface::XdgSurface,
    xdg_toplevel::{self, XdgToplevel},
};
use world::camera::transform::translate::{fit::window_fit, slot};
use world::comp::usable::Reserved;
use world::window::interface::data::WindowFullscreen;
use world::window::interface::record::window::LoopWindow;

fn window(h: &Harness, surface: &WlSurface) -> (SurfaceId, Window) {
    let handle = h.handle_of(surface);
    let id = h
        .comp()
        .registry
        .id_for_handle(&handle)
        .expect("registry identity");
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
        .expect("mapped window")
        .clone();
    (id, window)
}

fn maximize(h: &mut Harness, id: SurfaceId, window: &Window, enabled: bool) -> GeometryChange {
    let host = &mut h.wire.inner;
    geometry::set_maximized(&mut host.comp, &mut host.space.state, id, window, enabled)
}

fn refresh(h: &mut Harness, id: SurfaceId, window: &Window) -> GeometryChange {
    let host = &mut h.wire.inner;
    geometry::refresh_usable(
        &mut host.comp,
        &mut host.space.state,
        &[(id, window.clone())],
    )
}

fn bottom(h: &mut Harness, px: i32) {
    h.wire.inner.comp.reserved.insert(
        h.wire.inner.output.name(),
        Reserved {
            bottom: px,
            ..Reserved::default()
        },
    );
}

fn serial(h: &Harness, xdg: &XdgSurface) -> u32 {
    h.client
        .state
        .xdg_configures
        .iter()
        .rev()
        .find(|(id, _)| *id == protocol_id(xdg))
        .expect("received configure")
        .1
}

fn count(h: &Harness, top: &XdgToplevel) -> usize {
    h.client
        .state
        .toplevel_configures
        .iter()
        .filter(|event| event.toplevel == protocol_id(top))
        .count()
}

fn configured(h: &Harness, top: &XdgToplevel, size: (i32, i32), maximized: bool) {
    let event = h
        .client
        .state
        .toplevel_configures
        .iter()
        .rev()
        .find(|event| event.toplevel == protocol_id(top))
        .expect("toplevel configure");
    assert_eq!(event.size, size);
    assert_eq!(
        event
            .states
            .contains(&(xdg_toplevel::State::Maximized as u32)),
        maximized
    );
}

fn fit(h: &Harness, window: &Window) -> world::camera::transform::translate::fit::WindowFit {
    let content = window.geometry();
    window_fit(
        h.wire.inner.space.state.element_location(window).unwrap(),
        content,
        content.size,
        slot::decided_size(window).unwrap(),
        false,
    )
}

#[test]
fn delayed_ack_and_older_buffer_never_regress_the_decided_slot() {
    let mut h = Harness::new();
    let (surface, xdg, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    h.wire
        .inner
        .space
        .state
        .map_element(window.clone(), (32, 24), false);
    refresh(&mut h, id, &window);
    assert!(maximize(&mut h, id, &window, true).windows);
    h.roundtrip();
    configured(&h, &top, (1920, 1080), true);
    h.client.attach(&surface, 1920, 1080);
    h.roundtrip();
    assert_eq!(window.geometry().size, Size::from((1920, 1080)));
    let restore = h.comp().float_geometry(id).unwrap();
    h.client.state.hold_xdg_configures = true;

    let before = count(&h, &top);
    bottom(&mut h, 80);
    assert_eq!(
        refresh(&mut h, id, &window),
        GeometryChange {
            usable: true,
            windows: true
        }
    );
    h.roundtrip();
    assert_eq!(count(&h, &top), before + 1);
    configured(&h, &top, (1920, 1000), true);
    let older = serial(&h, &xdg);
    assert_eq!(slot::decided_size(&window), Some((1920, 1000).into()));
    assert_eq!(window.geometry().size, Size::from((1920, 1080)));
    let delayed_fit = fit(&h, &window);
    assert_eq!(
        delayed_fit.fit_sx, delayed_fit.fit_sy,
        "no resize gesture stretch"
    );
    assert_eq!(delayed_fit.fit_sx, 1.0);
    assert_eq!(
        delayed_fit.fit_surf,
        (0.0, -40.0),
        "old content centres in the smaller authoritative slot"
    );
    xdg.ack_configure(older);
    h.roundtrip();
    assert_eq!(slot::decided_size(&window), Some((1920, 1000).into()));
    assert_eq!(
        window.geometry().size,
        Size::from((1920, 1080)),
        "ACK alone is not a replacement buffer"
    );

    bottom(&mut h, 160);
    assert!(refresh(&mut h, id, &window).windows);
    h.roundtrip();
    configured(&h, &top, (1920, 920), true);
    h.client.attach(&surface, 1920, 1000); // legally ACKed older configure
    h.roundtrip();
    assert_eq!(window.geometry().size, Size::from((1920, 1000)));
    assert_eq!(slot::decided_size(&window), Some((1920, 920).into()));
    let before = count(&h, &top);
    assert_eq!(refresh(&mut h, id, &window), GeometryChange::default());
    h.roundtrip();
    assert_eq!(
        count(&h, &top),
        before,
        "no configure churn while newest ACK is held"
    );
    xdg.ack_configure(serial(&h, &xdg));
    h.client.attach(&surface, 1920, 920);
    h.roundtrip();
    assert_eq!(fit(&h, &window).fit_sx, 1.0);
    assert_eq!(fit(&h, &window).fit_surf, (0.0, 0.0));

    bottom(&mut h, 0);
    assert!(refresh(&mut h, id, &window).windows);
    h.roundtrip();
    configured(&h, &top, (1920, 1080), true);
    xdg.ack_configure(serial(&h, &xdg));
    h.client.attach(&surface, 1920, 1080);
    h.roundtrip();
    assert_eq!(h.comp().float_geometry(id), Some(restore.clone()));
    assert!(maximize(&mut h, id, &window, false).windows);
    h.roundtrip();
    configured(&h, &top, (640, 480), false);
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some(restore.location)
    );
    assert_eq!(slot::decided_size(&window), Some(restore.size));
    assert_eq!(h.comp().placement(id), world::comp::Placement::Floating);
}

fn output(name: &str, size: (i32, i32)) -> Output {
    let output = Output::new(
        name.into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "testkit".into(),
            model: "headless".into(),
            serial_number: name.into(),
        },
    );
    output.change_current_state(
        Some(Mode {
            size: size.into(),
            refresh: 60_000,
        }),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        None,
    );
    output
}

#[test]
fn explicit_maximise_output_retargets_without_losing_normal_geometry() {
    let mut h = Harness::new();
    let secondary = output("secondary", (1280, 800));
    h.wire.inner.space.state.map_output(&secondary, (1920, 0));
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    h.wire
        .inner
        .space
        .state
        .map_element(window.clone(), (32, 24), false);
    maximize(&mut h, id, &window, true);
    h.roundtrip();
    let restore = h.comp().float_geometry(id).unwrap();
    let host = &mut h.wire.inner;
    assert!(
        geometry::set_maximized_on(
            &mut host.comp,
            &mut host.space.state,
            id,
            &window,
            true,
            Some("secondary")
        )
        .windows
    );
    h.roundtrip();
    configured(&h, &top, (1280, 800), true);
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((1920, 0).into())
    );
    let retargeted = h.comp().float_geometry(id).unwrap();
    assert_eq!(
        (retargeted.location, retargeted.size),
        (restore.location, restore.size)
    );
    assert_eq!(retargeted, restore);
    assert_eq!(h.comp().placement(id).output(), Some("secondary"));
    let before = count(&h, &top);
    let host = &mut h.wire.inner;
    assert_eq!(
        geometry::set_maximized_on(
            &mut host.comp,
            &mut host.space.state,
            id,
            &window,
            true,
            Some("missing")
        ),
        GeometryChange::default()
    );
    h.roundtrip();
    assert_eq!(count(&h, &top), before);
    assert_eq!(h.comp().float_geometry(id), Some(retargeted));
    maximize(&mut h, id, &window, false);
    h.roundtrip();
    configured(&h, &top, (640, 480), false);
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((32, 24).into())
    );
}

#[test]
fn only_the_owning_output_reconfigures_and_restore_survives_output_loss() {
    let mut h = Harness::new();
    let secondary = output("secondary", (1280, 800));
    h.wire.inner.space.state.map_output(&secondary, (1920, 0));
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    h.wire
        .inner
        .space
        .state
        .map_element(window.clone(), (2000, 100), false);
    h.wire.inner.space.state.refresh();
    refresh(&mut h, id, &window);
    maximize(&mut h, id, &window, true);
    h.roundtrip();
    configured(&h, &top, (1280, 800), true);
    let restore = h.comp().float_geometry(id).unwrap();
    assert_eq!(restore.output, "secondary");
    let before = count(&h, &top);
    bottom(&mut h, 80); // unrelated primary output
    assert_eq!(
        refresh(&mut h, id, &window),
        GeometryChange {
            usable: true,
            windows: false
        }
    );
    h.roundtrip();
    assert_eq!(count(&h, &top), before);
    h.wire.inner.comp.reserved.insert(
        "secondary".into(),
        Reserved {
            bottom: 80,
            ..Reserved::default()
        },
    );
    assert!(refresh(&mut h, id, &window).windows);
    h.roundtrip();
    configured(&h, &top, (1280, 720), true);
    h.wire.inner.space.state.unmap_output(&secondary);
    assert!(refresh(&mut h, id, &window).windows);
    h.roundtrip();
    configured(&h, &top, (1920, 1000), true);
    let fallback = h.comp().float_geometry(id).unwrap();
    assert_eq!(
        (fallback.location, fallback.size),
        (restore.location, restore.size)
    );
    let primary = h.wire.inner.output.clone();
    h.wire.inner.space.state.unmap_output(&primary);
    let before = count(&h, &top);
    assert_eq!(
        refresh(&mut h, id, &window),
        GeometryChange {
            usable: true,
            windows: false
        }
    );
    h.roundtrip();
    assert_eq!(count(&h, &top), before);
    assert_eq!(h.comp().float_geometry(id), Some(fallback));
    h.wire.inner.space.state.map_output(&secondary, (1920, 0));
    assert!(refresh(&mut h, id, &window).windows);
    h.roundtrip();
    configured(&h, &top, (1280, 720), true);
    let recovered = h.comp().float_geometry(id).unwrap();
    assert_eq!(recovered, restore);
    maximize(&mut h, id, &window, false);
    h.roundtrip();
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((2000, 100).into())
    );
    configured(&h, &top, (640, 480), false);
}

#[test]
fn fullscreen_holds_geometry_until_exit_commit_then_uses_latest_work_area() {
    let mut h = Harness::new();
    let (surface, xdg, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    refresh(&mut h, id, &window);
    maximize(&mut h, id, &window, true);
    h.roundtrip();
    h.client.attach(&surface, 1920, 1080);
    h.roundtrip();
    let restore = h.comp().float_geometry(id).unwrap();
    window.set_fullscreen(Some(WindowFullscreen {
        restore_loc: (0, 0).into(),
        restore_size: (1920, 1080).into(),
    }));
    bottom(&mut h, 40);
    assert!(
        !refresh(&mut h, id, &window).windows,
        "compositor fullscreen record owns slot before protocol intent"
    );
    // Stage the real protocol fullscreen ownership. Loop-owned restore data is
    // separately fenced by the same production helper; no fake Loop is built.
    shell::set_fullscreen(&window, true);
    shell::send(&window);
    h.client.state.hold_xdg_configures = true;
    h.roundtrip();
    let before = count(&h, &top);
    bottom(&mut h, 80);
    assert!(
        !refresh(&mut h, id, &window).windows,
        "pending fullscreen owns slot"
    );
    h.roundtrip();
    assert_eq!(count(&h, &top), before);
    xdg.ack_configure(serial(&h, &xdg));
    h.client.attach(&surface, 1920, 1080);
    h.roundtrip();
    shell::set_fullscreen(&window, false);
    window.set_fullscreen(None);
    shell::send(&window);
    h.roundtrip();
    let before = count(&h, &top);
    bottom(&mut h, 160);
    assert!(
        !refresh(&mut h, id, &window).windows,
        "committed fullscreen retains ownership through delayed exit"
    );
    h.roundtrip();
    assert_eq!(count(&h, &top), before);
    assert_eq!(slot::decided_size(&window), Some((1920, 1080).into()));
    xdg.ack_configure(serial(&h, &xdg));
    h.client.attach(&surface, 1920, 1080);
    h.roundtrip();
    assert!(
        refresh(&mut h, id, &window).windows,
        "unchanged usable map must still reconcile exit"
    );
    h.roundtrip();
    configured(&h, &top, (1920, 920), true);
    let before = count(&h, &top);
    assert_eq!(refresh(&mut h, id, &window), GeometryChange::default());
    h.roundtrip();
    assert_eq!(count(&h, &top), before);
    assert_eq!(h.comp().float_geometry(id), Some(restore));
}

#[test]
fn unmaximise_restores_the_decided_size_when_client_geometry_lags() {
    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    h.wire
        .inner
        .space
        .state
        .map_element(window.clone(), (16, 24), false);
    slot::set_expected_size(&window, (800, 600).into());
    assert_eq!(window.geometry().size, Size::from((640, 480)));
    maximize(&mut h, id, &window, true);
    h.roundtrip();
    assert_eq!(
        h.comp().float_geometry(id).unwrap().size,
        Size::from((800, 600))
    );
    bottom(&mut h, 80);
    assert!(refresh(&mut h, id, &window).windows);
    h.roundtrip();
    maximize(&mut h, id, &window, false);
    h.roundtrip();
    configured(&h, &top, (800, 600), false);
    assert_eq!(slot::decided_size(&window), Some((800, 600).into()));
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((16, 24).into())
    );
}

#[test]
fn unmaximise_without_a_restore_record_clears_intent_without_moving_the_slot() {
    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    h.wire
        .inner
        .space
        .state
        .map_element(window.clone(), (16, 24), false);
    slot::set_expected_size(&window, (800, 600).into());
    shell::stage(&window, (800, 600).into(), false);
    shell::set_maximized(&window, true);
    shell::send(&window);
    h.roundtrip();
    configured(&h, &top, (800, 600), true);
    assert!(h.comp().float_geometry(id).is_none());

    assert_eq!(
        maximize(&mut h, id, &window, false),
        GeometryChange::default()
    );
    h.roundtrip();
    configured(&h, &top, (800, 600), false);
    assert_eq!(slot::decided_size(&window), Some((800, 600).into()));
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((16, 24).into())
    );
    assert!(h.comp().float_geometry(id).is_none());
    let before = count(&h, &top);
    maximize(&mut h, id, &window, false);
    h.roundtrip();
    assert_eq!(
        count(&h, &top),
        before + 1,
        "explicit requests retain their configure response"
    );
    configured(&h, &top, (800, 600), false);
    assert_eq!(slot::decided_size(&window), Some((800, 600).into()));
    bottom(&mut h, 80);
    let before = count(&h, &top);
    assert_eq!(
        refresh(&mut h, id, &window),
        GeometryChange {
            usable: true,
            windows: false
        }
    );
    h.roundtrip();
    assert_eq!(
        count(&h, &top),
        before,
        "work-area changes cannot recreate cleared intent"
    );
    assert_eq!(slot::decided_size(&window), Some((800, 600).into()));
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((16, 24).into())
    );
}

#[test]
fn refreshing_another_world_never_admits_its_window_into_this_space() {
    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    refresh(&mut h, id, &window);
    maximize(&mut h, id, &window, true);
    h.roundtrip();
    let restore = h.comp().float_geometry(id).unwrap();
    h.wire.inner.space.state.unmap_elem(&window);
    let mut other = smithay::desktop::Space::default();
    other.map_element(window.clone(), (10, 20), false);
    bottom(&mut h, 80);
    let before = count(&h, &top);
    assert_eq!(
        refresh(&mut h, id, &window),
        GeometryChange {
            usable: true,
            windows: false
        }
    );
    h.roundtrip();
    assert!(h.wire.inner.space.state.element_location(&window).is_none());
    assert_eq!(other.element_location(&window), Some((10, 20).into()));
    assert_eq!(count(&h, &top), before);
    assert_eq!(h.comp().float_geometry(id), Some(restore));
    // Once its world is hosted again, the unchanged usable map still reflows.
    other.unmap_elem(&window);
    h.wire
        .inner
        .space
        .state
        .map_element(window.clone(), (32, 24), false);
    assert!(refresh(&mut h, id, &window).windows);
    h.roundtrip();
    configured(&h, &top, (1920, 1000), true);
}

fn tile(
    h: &mut Harness,
    id: SurfaceId,
    window: &Window,
    slot: world::comp::TileSlot,
) -> GeometryChange {
    let host = &mut h.wire.inner;
    geometry::set_placement(
        &mut host.comp,
        &mut host.space.state,
        id,
        window,
        world::comp::Placement::Tiled {
            slot,
            output: host.output.name(),
        },
    )
}

#[test]
fn usable_changes_refit_tiles_and_map_saved_float_centres_without_resizing() {
    use world::comp::{TileSlot, Placement};
    let mut h = Harness::new();
    let (surface, _, _) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    h.wire.inner.space.state.map_element(window.clone(), (640, 300), false);
    tile(&mut h, id, &window, TileSlot::BottomRight);
    h.wire.inner.comp.reserved.insert("testkit-0".into(), Reserved {
        left: 100, right: 100, top: 60, bottom: 120,
    });
    assert!(refresh(&mut h, id, &window).windows);
    assert_eq!(h.wire.inner.space.state.element_location(&window), Some((960, 510).into()));
    assert_eq!(slot::decided_size(&window), Some((860, 450).into()));
    let saved = h.comp().float_geometry(id).unwrap();
    assert_eq!(saved.location, (640, 270).into());
    assert_eq!(saved.size, (640, 480).into());
    assert_eq!(refresh(&mut h, id, &window), GeometryChange::default());
    maximize(&mut h, id, &window, false);
    assert_eq!(h.comp().placement(id), Placement::Floating);
    assert_eq!(h.wire.inner.space.state.element_location(&window), Some(saved.location));
    assert_eq!(slot::decided_size(&window), Some(saved.size));
}

#[test]
fn removing_an_output_restores_tiles_and_maximised_windows_on_a_survivor() {
    use world::comp::{Placement, TileSlot};
    for tiled in [true, false] {
        let mut h = Harness::two_outputs();
        let (surface, _, top) = h.mapped_toplevel(640, 480);
        let (id, window) = window(&h, &surface);
        h.wire.inner.space.state.map_element(window.clone(), (2043, 87), false);
        h.wire.inner.space.state.refresh();
        refresh(&mut h, id, &window);
        if tiled { tile(&mut h, id, &window, TileSlot::Right); }
        else { maximize(&mut h, id, &window, true); }
        assert_eq!(h.comp().placement(id).output(), Some("testkit-1"));
        let secondary = h.wire.inner.space.state.outputs().find(|o| o.name() == "testkit-1").unwrap().clone();
        h.wire.inner.space.state.unmap_output(&secondary);
        assert!(refresh(&mut h, id, &window).windows);
        assert_eq!(h.comp().placement(id), Placement::Floating);
        let saved = h.comp().float_geometry(id).unwrap();
        assert_eq!(saved.output, "testkit-0");
        assert_eq!(saved.location, (123, 87).into());
        assert_eq!(saved.size, (640, 480).into());
        assert_eq!(h.wire.inner.space.state.element_location(&window), Some(saved.location));
        assert_eq!(slot::decided_size(&window), Some(saved.size));
        h.roundtrip();
        configured(&h, &top, (640, 480), false);
        assert_eq!(refresh(&mut h, id, &window), GeometryChange::default());
    }
}

#[test]
fn sending_a_tile_keeps_its_slot_and_uses_the_destination_workspace_split() {
    use policy::workspaces::{DefaultOutput, WorkspaceTarget};
    use world::comp::TileSlot;
    let mut h = Harness::new();
    h.wire.inner.comp.default_output = Some(DefaultOutput { key: "o_testkit_0".into(), name: "testkit-0".into() });
    let (surface, _, _) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    tile(&mut h, id, &window, TileSlot::BottomRight);
    let saved = h.comp().float_geometry(id);
    let placement = h.comp().placement(id);
    *h.wire.inner.comp.tile_split("testkit-0", 2) = world::comp::TileSplit { x: 0.3, y: 0.7 };
    let comp = &mut h.wire.inner.comp;
    let output = comp.default_output.clone();
    comp.workspaces.move_window(&mut comp.registry, output.as_ref(), id, WorkspaceTarget::Index(2), None).unwrap();
    assert!(h.comp().hidden_id(id));
    assert!(refresh(&mut h, id, &window).windows);
    assert_eq!(h.comp().placement(id), placement);
    assert_eq!(h.comp().float_geometry(id), saved);
    assert_eq!(h.wire.inner.space.state.element_location(&window), Some((576, 756).into()));
    assert_eq!(slot::decided_size(&window), Some((1344, 324).into()));
    let comp = &mut h.wire.inner.comp;
    comp.workspaces.switch(&comp.registry, output.as_ref(), None, WorkspaceTarget::Index(2), false, None).unwrap();
    assert!(!h.comp().hidden_id(id));
    assert_eq!(refresh(&mut h, id, &window), GeometryChange::default());
    assert_eq!(h.comp().tile_split_for("testkit-0", 1), world::comp::TileSplit::default());
}

#[test]
fn unmaximise_during_fullscreen_exit_ack_applies_geometry_and_a_configure() {
    use world::comp::{Placement, TileSlot};
    for tiled in [true, false] {
        let mut h = Harness::new();
        let (surface, xdg, top) = h.mapped_toplevel(640, 480);
        let (id, window) = window(&h, &surface);
        h.wire.inner.space.state.map_element(window.clone(), (123, 87), false);
        if tiled { tile(&mut h, id, &window, TileSlot::Left); }
        else { maximize(&mut h, id, &window, true); }
        let saved = h.comp().float_geometry(id).unwrap();
        fullscreen(&mut h, &window, true);
        h.roundtrip();
        h.client.commit(&surface);
        h.roundtrip();
        h.client.state.hold_xdg_configures = true;
        fullscreen(&mut h, &window, false);
        assert!(window.fullscreen().is_none());
        assert!(window.toplevel().unwrap().with_committed_state(|s| s.unwrap().states.contains(xdg_toplevel::State::Fullscreen)));
        h.roundtrip();
        let before = count(&h, &top);
        assert!(maximize(&mut h, id, &window, false).windows);
        h.roundtrip();
        assert_eq!(count(&h, &top), before + 1);
        configured(&h, &top, (640, 480), false);
        assert_eq!(h.comp().placement(id), Placement::Floating);
        assert_eq!(h.wire.inner.space.state.element_location(&window), Some(saved.location));
        assert_eq!(slot::decided_size(&window), Some(saved.size));
        xdg.ack_configure(serial(&h, &xdg));
        h.client.commit(&surface);
        h.roundtrip();
        assert_eq!(refresh(&mut h, id, &window), GeometryChange::default());
    }
}

#[test]
fn tiled_title_bands_keep_square_corners_and_action_hit_targets() {
    use decor::layout::{CaptionButton, ChromePart, vec2};
    use world::comp::TileSlot;

    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    top.set_app_id("dev.mixos.testkit".into());
    let mut theme = decor::ChromeTheme::from_source(decor::ChromeStyle::Mac, None);
    theme.band.place = design::ActionsPlace::Titlebar;
    theme.menu_style = design::MenuStyle::Titlebar;
    decor::window::install(theme.clone());
    window.toplevel().unwrap().with_pending_state(|state| {
        state.decoration_mode = Some(DecorationMode::ServerSide);
    });
    window.toplevel().unwrap().send_configure();
    h.roundtrip();
    h.client.commit(&surface);
    h.roundtrip();
    decor::window::menu_services(std::collections::BTreeSet::from(["testkit".into()]));
    assert!(decor::window::update_actions(
        "testkit",
        serde_json::from_value(serde_json::json!({
            "revision": 7,
            "items": [{
                "command": "file.save", "icon": "save", "tooltip": "Save",
                "enabled": true, "toggled": false
            }]
        }))
        .unwrap(),
    ));
    for tiled in [TileSlot::Left, TileSlot::BottomRight] {
        assert!(tile(&mut h, id, &window, tiled).windows);
        h.roundtrip();
        assert!(decor::window::constrained(&window));
        let size = slot::decided_size(&window).unwrap();
        let layout = decor::window::layout(&window, vec2(size.w as f32, size.h as f32)).unwrap();
        assert_eq!(layout.action_buttons.len(), 1);
        let (index, rect) = layout.action_buttons[0];
        let extents = decor::window::extents(&window).unwrap();
        let point = (
            (rect.center().x - extents.left) as f64,
            (rect.center().y - extents.top) as f64,
        );
        let hit = decor::window::hit(&window, point, size).unwrap();
        assert_eq!(
            hit.part,
            ChromePart::Button(CaptionButton::Action(7, index))
        );
        assert_eq!(decor::seat::press(&window, hit, point, 100), None);
        assert_eq!(
            decor::seat::release(Some((&window, hit))).unwrap().1,
            decor::Intent::Action { revision: 7, index },
        );
        let state = decor::ChromeState {
            content_size: (size.w, size.h),
            maximized: decor::window::constrained(&window),
            actions: decor::window::actions(&window),
            ..Default::default()
        };
        let image = decor::raster::titlebar(&theme.deco, &layout, &state, 1.0, None);
        assert!(image.at(0, 0)[3] > 0.9);
    }
    decor::seat::forget(&window);
    decor::window::menu_services(Default::default());
}

#[test]
fn tiled_maximised_and_unmaximised_restore_the_pre_tile_floating_geometry() {
    use world::comp::{Placement, TileSlot};
    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    h.wire
        .inner
        .space
        .state
        .map_element(window.clone(), (32, 24), false);
    assert!(tile(&mut h, id, &window, TileSlot::Left).windows);
    let saved = h.comp().float_geometry(id).unwrap();
    assert_eq!(
        (saved.location, saved.size),
        ((32, 24).into(), (640, 480).into())
    );
    assert!(decor::window::constrained(&window));
    assert!(!h.comp().placement(id).maximized());
    h.roundtrip();
    configured(&h, &top, (960, 1080), false);
    tile(&mut h, id, &window, TileSlot::BottomRight);
    assert_eq!(h.comp().float_geometry(id), Some(saved.clone()));
    maximize(&mut h, id, &window, true);
    assert!(h.comp().placement(id).maximized());
    assert_eq!(h.comp().float_geometry(id), Some(saved.clone()));
    // Maximised -> tiled must keep the same pre-tile geometry too.
    tile(&mut h, id, &window, TileSlot::Top);
    assert_eq!(h.comp().float_geometry(id), Some(saved.clone()));
    maximize(&mut h, id, &window, true);
    maximize(&mut h, id, &window, false);
    h.roundtrip();
    assert_eq!(h.comp().placement(id), Placement::Floating);
    assert!(!decor::window::constrained(&window));
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some(saved.location)
    );
    assert_eq!(slot::decided_size(&window), Some(saved.size));
    configured(&h, &top, (640, 480), false);
    // Kept saved geometry is never itself evidence of maximised state.
    assert_eq!(h.comp().float_geometry(id), Some(saved));
    h.wire
        .inner
        .space
        .state
        .map_element(window.clone(), (80, 90), false);
    slot::set_expected_size(&window, (800, 600).into());
    tile(&mut h, id, &window, TileSlot::Right);
    let saved = h.comp().float_geometry(id).unwrap();
    assert_eq!(
        (saved.location, saved.size),
        ((80, 90).into(), (800, 600).into())
    );
}

#[test]
fn tiled_window_refits_after_a_usable_area_change_without_configure_churn() {
    use world::comp::TileSlot;
    let mut h = Harness::new();
    let (surface, _, top) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    tile(&mut h, id, &window, TileSlot::BottomRight);
    let saved = h.comp().float_geometry(id);
    h.roundtrip();
    bottom(&mut h, 81);
    assert!(refresh(&mut h, id, &window).windows);
    h.roundtrip();
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((960, 500).into())
    );
    assert_eq!(slot::decided_size(&window), Some((960, 499).into()));
    configured(&h, &top, (960, 499), false);
    assert_eq!(h.comp().float_geometry(id), saved);
    let before = count(&h, &top);
    assert_eq!(refresh(&mut h, id, &window), GeometryChange::default());
    h.roundtrip();
    assert_eq!(count(&h, &top), before);
}

#[test]
fn tile_anchors_min_size_and_centres_max_size_clients_without_configure_churn() {
    use world::comp::TileSlot;
    for (min, max, expected_size, expected_origin) in [
        ((1200, 1200), (0, 0), (1200, 1200), (0, 0)),
        ((0, 0), (600, 400), (600, 400), (180, 340)),
    ] {
        let mut h = Harness::new();
        let (surface, _, top) = h.mapped_toplevel(640, 480);
        let (id, window) = window(&h, &surface);
        top.set_min_size(min.0, min.1);
        top.set_max_size(max.0, max.1);
        h.client.commit(&surface);
        h.roundtrip();
        tile(&mut h, id, &window, TileSlot::Left);
        h.roundtrip();
        assert_eq!(
            h.wire.inner.space.state.element_location(&window),
            Some(expected_origin.into())
        );
        assert_eq!(slot::decided_size(&window), Some(expected_size.into()));
        configured(&h, &top, expected_size, false);
        refresh(&mut h, id, &window); // first work-area observation
        let before = count(&h, &top);
        for _ in 0..3 {
            assert!(!refresh(&mut h, id, &window).windows);
            h.roundtrip();
        }
        assert_eq!(
            count(&h, &top),
            before,
            "hint-clamped tile cannot re-fit forever"
        );
    }
}

fn panelled_output(h: &mut Harness) {
    h.wire.inner.output.change_current_state(
        Some(Mode {
            size: (1536, 864).into(),
            refresh: 60_000,
        }),
        None,
        None,
        None,
    );
    h.wire.inner.comp.reserved.insert(
        h.wire.inner.output.name(),
        Reserved {
            top: 28,
            bottom: 52,
            ..Reserved::default()
        },
    );
}

#[test]
fn all_tiles_anchor_oversized_clients_before_and_after_their_min_size_commit() {
    use world::comp::TileSlot;
    for late_min in [false, true] {
        for (tiled, origin, size) in [
            (TileSlot::Left, (0, 28), (820, 784)),
            (TileSlot::Right, (716, 28), (820, 784)),
            (TileSlot::Top, (0, 28), (1536, 520)),
            (TileSlot::Bottom, (0, 292), (1536, 520)),
            (TileSlot::TopLeft, (0, 28), (820, 520)),
            (TileSlot::TopRight, (716, 28), (820, 520)),
            (TileSlot::BottomLeft, (0, 292), (820, 520)),
            (TileSlot::BottomRight, (716, 292), (820, 520)),
        ] {
            let mut h = Harness::new();
            panelled_output(&mut h);
            let (surface, _, top) = h.mapped_toplevel(640, 480);
            let (id, window) = window(&h, &surface);
            if !late_min {
                top.set_min_size(820, 520);
                h.client.commit(&surface);
                h.roundtrip();
            }
            tile(&mut h, id, &window, tiled);
            h.roundtrip();
            let saved = h.comp().float_geometry(id);
            if late_min {
                let area = tiled.rectangle(
                    smithay::utils::Rectangle::new((0, 28).into(), (1536, 784).into()),
                    world::comp::TileSplit::default(),
                );
                configured(&h, &top, (area.size.w, area.size.h), false);
                // The real late-client path: ACK the tile configure, then
                // commit its minimum and a larger buffer through Wayland.
                top.set_min_size(820, 520);
            }
            h.client.attach(&surface, size.0, size.1);
            h.roundtrip();
            refresh(&mut h, id, &window);
            h.roundtrip();
            assert_eq!(window.geometry().size, size.into());
            assert_eq!(slot::decided_size(&window), Some(size.into()));
            assert_eq!(
                h.wire.inner.space.state.element_location(&window),
                Some(origin.into()),
                "{tiled:?}, late minimum: {late_min}"
            );
            assert_eq!(h.comp().placement(id).name(), tiled.name());
            assert_eq!(h.comp().float_geometry(id), saved);
            assert_eq!(
                h.comp().tile_split_for("testkit-0", 1),
                world::comp::TileSplit::default()
            );
            let before = count(&h, &top);
            assert!(!refresh(&mut h, id, &window).windows);
            h.roundtrip();
            assert_eq!(count(&h, &top), before);
        }
    }
}

#[test]
fn tiles_larger_than_the_usable_output_start_at_its_top_left() {
    use world::comp::TileSlot;
    for tiled in TileSlot::ALL {
        let mut h = Harness::new();
        panelled_output(&mut h);
        // Include a negative output origin and side panel, so clamping must
        // use the usable rectangle rather than zero or the raw output.
        let output = h.wire.inner.output.clone();
        h.wire.inner.space.state.map_output(&output, (-300, 140));
        h.wire
            .inner
            .comp
            .reserved
            .get_mut(&output.name())
            .unwrap()
            .left = 61;
        let (surface, _, top) = h.mapped_toplevel(640, 480);
        let (id, window) = window(&h, &surface);
        top.set_min_size(1700, 1000);
        h.client.commit(&surface);
        h.roundtrip();
        tile(&mut h, id, &window, tiled);
        h.roundtrip();
        h.client.attach(&surface, 1700, 1000);
        h.roundtrip();
        refresh(&mut h, id, &window);
        assert_eq!(slot::decided_size(&window), Some((1700, 1000).into()));
        assert_eq!(
            h.wire.inner.space.state.element_location(&window),
            Some((-239, 168).into()),
            "{tiled:?}"
        );
    }
}

#[test]
fn oversized_tile_refits_after_panel_and_output_changes() {
    use world::comp::TileSlot;
    let mut h = Harness::new();
    panelled_output(&mut h);
    let (surface, _, top) = h.mapped_toplevel(640, 500);
    let (id, window) = window(&h, &surface);
    top.set_min_size(0, 500);
    h.client.commit(&surface);
    h.roundtrip();
    for (tiled, origin, size) in [
        (TileSlot::Top, (0, 28), (1536, 500)),
        (TileSlot::BottomLeft, (0, 312), (768, 500)),
    ] {
        tile(&mut h, id, &window, tiled);
        assert_eq!(
            h.wire.inner.space.state.element_location(&window),
            Some(origin.into())
        );
        assert_eq!(slot::decided_size(&window), Some(size.into()));
    }
    bottom(&mut h, 80);
    assert!(refresh(&mut h, id, &window).windows);
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((0, 284).into())
    );
    h.wire.inner.output.change_current_state(
        Some(Mode {
            size: (1280, 720).into(),
            refresh: 60_000,
        }),
        None,
        None,
        None,
    );
    assert!(refresh(&mut h, id, &window).windows);
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((0, 140).into())
    );
    assert_eq!(slot::decided_size(&window), Some((640, 500).into()));
    assert_eq!(h.comp().placement(id).name(), "bottom_left");
    assert!(!refresh(&mut h, id, &window).windows);
}

#[test]
fn maximized_clients_with_size_limits_anchor_to_the_usable_top_left() {
    for (min, max, size) in [
        ((1700, 1000), (0, 0), (1700, 1000)),
        ((0, 0), (800, 500), (800, 500)),
    ] {
        let mut h = Harness::new();
        panelled_output(&mut h);
        let (surface, _, top) = h.mapped_toplevel(640, 480);
        let (id, window) = window(&h, &surface);
        // A client may announce new limits after it was maximised.
        maximize(&mut h, id, &window, true);
        h.roundtrip();
        configured(&h, &top, (1536, 784), true);
        top.set_min_size(min.0, min.1);
        top.set_max_size(max.0, max.1);
        h.client.attach(&surface, size.0, size.1);
        h.roundtrip();
        refresh(&mut h, id, &window);
        h.roundtrip();
        configured(&h, &top, size, true);
        assert_eq!(slot::decided_size(&window), Some(size.into()));
        assert_eq!(
            h.wire.inner.space.state.element_location(&window),
            Some((0, 28).into())
        );
        assert!(!refresh(&mut h, id, &window).windows);
    }
}

#[test]
fn fullscreen_roundtrip_keeps_tiled_and_maximised_placement_and_recomputes_its_area() {
    use world::comp::{Placement, TileSlot};
    use world::window::interface::draw::fullscreen::fullscreen_set_in_space;
    for tiled in [false, true] {
        let mut h = Harness::new();
        let (surface, _, top) = h.mapped_toplevel(640, 480);
        let (id, window) = window(&h, &surface);
        if tiled {
            tile(&mut h, id, &window, TileSlot::BottomRight);
        } else {
            maximize(&mut h, id, &window, true);
        }
        h.roundtrip();
        let placement = h.comp().placement(id);
        let saved = h.comp().float_geometry(id);
        {
            let host = &mut h.wire.inner;
            assert!(fullscreen_set_in_space(
                &host.comp,
                &mut host.space.state,
                &window,
                true,
                Some(((0, 0).into(), (1920, 1080).into())),
                false
            ));
        }
        h.roundtrip();
        assert_eq!(h.comp().placement(id), placement);
        assert_eq!(h.comp().float_geometry(id), saved);
        bottom(&mut h, 80);
        assert!(!refresh(&mut h, id, &window).windows);
        // Change it again without a refresh: exit must calculate, not use comp.usable.
        bottom(&mut h, 160);
        h.client.state.hold_xdg_configures = true;
        {
            let host = &mut h.wire.inner;
            assert!(fullscreen_set_in_space(
                &host.comp,
                &mut host.space.state,
                &window,
                false,
                None,
                false
            ));
        }
        let (origin, size) = if tiled {
            ((960, 460), (960, 460))
        } else {
            ((0, 0), (1920, 920))
        };
        assert_eq!(
            h.wire.inner.space.state.element_location(&window),
            Some(origin.into())
        );
        assert_eq!(slot::decided_size(&window), Some(size.into()));
        assert_eq!(h.comp().placement(id), placement);
        assert_eq!(h.comp().float_geometry(id), saved);
        assert!(matches!(
            placement,
            Placement::Maximized { .. } | Placement::Tiled { .. }
        ));
        h.roundtrip();
        configured(&h, &top, size, !tiled);
    }
}

#[test]
fn fullscreen_exit_reinstates_the_chrome_inset_before_the_exit_ack() {
    use world::window::interface::draw::fullscreen::fullscreen_set_in_space;
    let mut h = Harness::new();
    let (surface, _, _) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
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
    let extents = decor::window::extents(&window).unwrap();
    maximize(&mut h, id, &window, true);
    {
        let host = &mut h.wire.inner;
        fullscreen_set_in_space(
            &host.comp,
            &mut host.space.state,
            &window,
            true,
            None,
            false,
        );
    }
    h.roundtrip();
    h.client.commit(&surface);
    h.roundtrip();
    assert!(
        decor::window::extents(&window).is_none(),
        "committed fullscreen hides chrome"
    );
    bottom(&mut h, 80);
    {
        let host = &mut h.wire.inner;
        fullscreen_set_in_space(
            &host.comp,
            &mut host.space.state,
            &window,
            false,
            None,
            false,
        );
    }
    let expected = decor::window::inset(
        smithay::utils::Rectangle::new((0, 0).into(), (1920, 1000).into()),
        extents,
    );
    assert_eq!(slot::decided_size(&window), Some(expected.size));
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some(expected.loc)
    );
}

fn fullscreen(h: &mut Harness, window: &Window, enabled: bool) {
    let host = &mut h.wire.inner;
    assert!(
        world::window::interface::draw::fullscreen::fullscreen_set_in_space(
            &host.comp,
            &mut host.space.state,
            window,
            enabled,
            enabled.then_some(((0, 0).into(), (1920, 1080).into())),
            false,
        )
    );
}

#[test]
fn unmaximise_during_fullscreen_restores_pre_maximise_floating_geometry_on_exit() {
    use dispatcher::wire::trait_::surface_event::WindowRequest;
    use world::comp::{Placement, TileSlot};

    for tiled in [false, true] {
        for client_request in [false, true] {
            let mut h = Harness::new();
            let (surface, xdg, top) = h.mapped_toplevel(640, 480);
            let (id, window) = window(&h, &surface);
            h.wire
                .inner
                .space
                .state
                .map_element(window.clone(), (123, 57), false);
            bottom(&mut h, 80);
            if tiled {
                tile(&mut h, id, &window, TileSlot::BottomRight);
            } else {
                maximize(&mut h, id, &window, true);
            }
            let saved = h.comp().float_geometry(id).unwrap();
            fullscreen(&mut h, &window, true);
            h.roundtrip();
            h.client.commit(&surface);
            h.roundtrip();

            if client_request {
                top.unset_maximized();
                h.roundtrip();
                assert_eq!(
                    h.wire.inner.comp.take_requests(),
                    vec![(id, WindowRequest::Maximize(false))]
                );
            }
            // Both request paths reach this same executor after policy admission.
            assert_eq!(
                maximize(&mut h, id, &window, false),
                GeometryChange::default()
            );
            assert_eq!(h.comp().placement(id), Placement::Floating);
            assert_eq!(h.comp().float_geometry(id), Some(saved.clone()));
            assert_eq!(slot::decided_size(&window), Some((1920, 1080).into()));
            assert_eq!(
                h.wire.inner.space.state.element_location(&window),
                Some((0, 0).into())
            );

            h.client.state.hold_xdg_configures = true;
            fullscreen(&mut h, &window, false);
            assert_eq!(
                h.wire.inner.space.state.element_location(&window),
                Some(saved.location)
            );
            assert_eq!(slot::decided_size(&window), Some(saved.size));
            assert_eq!(h.comp().float_geometry(id), Some(saved.clone()));
            h.roundtrip();
            configured(&h, &top, (640, 480), false);
            xdg.ack_configure(serial(&h, &xdg));
            h.client.commit(&surface);
            h.roundtrip();
            h.client.state.hold_xdg_configures = false;

            // A later maximise must save A again, never the old maximised slot M.
            assert!(maximize(&mut h, id, &window, true).windows);
            assert_eq!(h.comp().float_geometry(id), Some(saved.clone()));
            assert!(maximize(&mut h, id, &window, false).windows);
            assert_eq!(
                h.wire.inner.space.state.element_location(&window),
                Some(saved.location)
            );
            assert_eq!(slot::decided_size(&window), Some(saved.size));
        }
    }
}

#[test]
fn maximise_during_fullscreen_reapplies_new_placement_and_preserves_floating_geometry() {
    use world::comp::{Placement, TileSlot};

    for tiled in [false, true] {
        let mut h = Harness::new();
        let (surface, xdg, top) = h.mapped_toplevel(640, 480);
        let (id, window) = window(&h, &surface);
        h.wire
            .inner
            .space
            .state
            .map_element(window.clone(), (123, 57), false);
        fullscreen(&mut h, &window, true);
        h.roundtrip();
        h.client.commit(&surface);
        h.roundtrip();

        let change = if tiled {
            tile(&mut h, id, &window, TileSlot::BottomRight)
        } else {
            top.set_maximized();
            h.roundtrip();
            assert_eq!(
                h.wire.inner.comp.take_requests(),
                vec![(
                    id,
                    dispatcher::wire::trait_::surface_event::WindowRequest::Maximize(true)
                )]
            );
            maximize(&mut h, id, &window, true)
        };
        assert_eq!(change, GeometryChange::default());
        let saved = h.comp().float_geometry(id).unwrap();
        assert_eq!(
            (saved.location, saved.size),
            ((123, 57).into(), (640, 480).into())
        );
        let placement = h.comp().placement(id);
        assert!(placement.constrained());
        assert_eq!(slot::decided_size(&window), Some((1920, 1080).into()));

        // Exit computes the new placement against the latest work area, even
        // without a refresh and while its configure ACK is held.
        bottom(&mut h, 160);
        h.client.state.hold_xdg_configures = true;
        fullscreen(&mut h, &window, false);
        let (origin, size) = if tiled {
            ((960, 460), (960, 460))
        } else {
            ((0, 0), (1920, 920))
        };
        assert_eq!(
            h.wire.inner.space.state.element_location(&window),
            Some(origin.into())
        );
        assert_eq!(slot::decided_size(&window), Some(size.into()));
        assert_eq!(h.comp().placement(id), placement);
        assert_eq!(h.comp().float_geometry(id), Some(saved.clone()));
        h.roundtrip();
        configured(&h, &top, size, !tiled);
        xdg.ack_configure(serial(&h, &xdg));
        h.client.commit(&surface);
        h.roundtrip();
        assert!(maximize(&mut h, id, &window, false).windows);
        assert_eq!(h.comp().placement(id), Placement::Floating);
        assert_eq!(
            h.wire.inner.space.state.element_location(&window),
            Some(saved.location)
        );
        assert_eq!(slot::decided_size(&window), Some(saved.size));
    }
}

#[test]
fn unchanged_floating_placement_exits_fullscreen_at_its_current_geometry() {
    let mut h = Harness::new();
    let (surface, _, _) = h.mapped_toplevel(640, 480);
    let (id, window) = window(&h, &surface);
    maximize(&mut h, id, &window, true);
    maximize(&mut h, id, &window, false);
    let old_saved = h.comp().float_geometry(id);
    h.wire
        .inner
        .space
        .state
        .map_element(window.clone(), (123, 57), false);
    slot::set_expected_size(&window, (800, 600).into());
    fullscreen(&mut h, &window, true);
    h.roundtrip();
    fullscreen(&mut h, &window, false);
    assert_eq!(
        h.wire.inner.space.state.element_location(&window),
        Some((123, 57).into())
    );
    assert_eq!(slot::decided_size(&window), Some((800, 600).into()));
    assert_eq!(h.comp().float_geometry(id), old_saved);
}
