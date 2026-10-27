// SPDX-License-Identifier: MIT OR Apache-2.0
//! Accessible map nodes, keyboard focus and native tree selection.
use egui_kittest::{Harness, kittest::Queryable};
use inspector::{Effect, Engine, Snapshot, model::Node};
use serde_json::json;
use toolkit::Theme;
use workshop::{actions, label, shell::apply_ui, strings, view::view};

fn clock() -> u64 {
    100_000
}
fn fixture() -> Engine {
    let mut engine = Engine::new(label);
    let Some(Effect::Discover { ticket }) = engine.take_effects().pop() else {
        panic!("discovery")
    };
    engine.discovered(
        ticket,
        Snapshot {
            nodes: vec![
                Node {
                    name: "alpha".into(),
                    local: true,
                    ..Default::default()
                },
                Node {
                    name: "beta".into(),
                    ..Default::default()
                },
                Node {
                    name: "gamma".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    );
    engine.take_effects();
    engine.ui.row_key = Some("nodes:mesh".into());
    engine.workshop.mesh = inspector::mesh::Mesh::new("alpha", "workshop", clock);
    engine.workshop.mesh.set_nodes(
        &json!({"name":"alpha","wg_ip":"192.0.2.1"}),
        &json!({"peers":[
            {"name":"beta","wg_ip":"192.0.2.2"},{"name":"gamma","wg_ip":"192.0.2.3"}
        ]}),
    );
    engine.workshop.mesh.set_observing(true, None);
    engine.workshop.subscription = Some("live".into());
    engine
        .workshop
        .mesh
        .set_probe(&json!({"gamma":{"ok":false,"rc":10,"at":95_000}}), clock());
    engine
}

fn status_harness(engine: Engine) -> Harness<'static, Vec<workshop::view::UiEvent>> {
    let registry = actions::registry();
    let strings = strings();
    let mut h = Harness::builder()
        .with_context_setup(|ctx| toolkit::install(ctx, &Theme::embedded()))
        .with_size(egui::vec2(1400.0, 1100.0))
        .build_ui_state(
            move |ui, events: &mut Vec<workshop::view::UiEvent>| {
                events.extend(view(ui, &engine, &registry, &strings));
            },
            Vec::new(),
        );
    h.run();
    h
}

#[test]
fn lossy_observation_is_visible_even_when_only_own_traffic_was_dropped() {
    let mut engine = fixture();
    engine.workshop.mesh.add_event_at(
        &json!({"direction":"local","from":"workshop","dropped_count":7}),
        clock(),
    );
    let h = status_harness(engine);
    assert!(
        h.query_by_label(&workshop::label_with(
            "mesh-ring-dropped",
            &[("count", "7")]
        ))
        .is_some()
    );
    assert!(h.query_by_label(&label("mesh-observe-live")).is_some());
}

#[test]
fn unknown_traffic_is_counted_beside_the_map_without_a_node_widget() {
    let mut engine = fixture();
    engine.workshop.mesh.add_event_at(
        &json!({"direction":"mesh_in","from":"noded","message_type":"request",
        "ts":"1970-01-01T00:01:10.000Z"}),
        clock(),
    );
    let h = status_harness(engine);
    assert!(h.query_by_label("?").is_none());
    assert!(
        h.query_by_label(&workshop::label_with(
            "mesh-unnamed",
            &[("out", "0"), ("in", "1")]
        ))
        .is_some()
    );
}

#[test]
fn quiet_live_observation_and_disconnected_observation_have_distinct_status() {
    fn later() -> u64 {
        145_000
    }
    for disconnected in [false, true] {
        let mut engine = fixture();
        // An excluded event still demonstrates that the stream delivered data.
        engine.workshop.mesh.add_event_at(
            &json!({"direction":"local","from":"workshop",
            "ts":"1970-01-01T00:01:10.000Z"}),
            clock(),
        );
        engine.workshop.mesh.clock = later;
        if disconnected {
            engine.delivery(inspector::bus::Delivery::Disconnected);
        }
        let h = status_harness(engine);
        assert!(
            h.query_by_label(&label(if disconnected {
                "mesh-observe-stopped"
            } else {
                "mesh-observe-live"
            }))
            .is_some()
        );
        assert!(
            h.query_by_label(&workshop::label_with("mesh-last-observe", &[("age", "45")]))
                .is_some()
        );
        if !disconnected {
            assert!(
                h.query_by_label(&workshop::label_with(
                    "mesh-observe-unavailable",
                    &[("reason", &label("mesh-waiting"))]
                ))
                .is_none()
            );
        }
    }
    let h = status_harness(fixture());
    assert!(h.query_by_label(&label("mesh-observe-live")).is_some());
    assert!(h.query_by_label(&label("mesh-no-observe-events")).is_some());
}

#[test]
fn mesh_details_never_draws_payload_or_a_body_inspector() {
    let mut engine = fixture();
    engine.ui.body = "DRAFT_BODY_ONLY_MARKER".into();
    engine.workshop.mesh.add_event_at(
        &json!({"direction":"mesh_in","peer":"beta","command":"noded.info",
        "ts":"1970-01-01T00:01:10.000Z","body":"BODY_ONLY_MARKER",
        "payload":{"data":"PAYLOAD_ONLY_MARKER"}}),
        clock(),
    );
    let h = status_harness(engine);
    for text in [
        "DRAFT_BODY_ONLY_MARKER",
        "BODY_ONLY_MARKER",
        "PAYLOAD_ONLY_MARKER",
        &label("body"),
        &label("reply"),
    ] {
        assert!(
            h.query_by_label(text).is_none(),
            "unexpected body inspector: {text}"
        );
    }
}

#[test]
fn labelled_map_node_click_selects_its_tree_row() {
    let registry = actions::registry();
    let strings = strings();
    let mut h = Harness::builder()
        .with_context_setup(|ctx| toolkit::install(ctx, &Theme::embedded()))
        .with_size(egui::vec2(1100.0, 900.0))
        .build_ui_state(
            move |ui, engine: &mut Engine| {
                let events = view(ui, engine, &registry, &strings);
                apply_ui(engine, &registry, events);
            },
            fixture(),
        );
    h.run();
    h.get_by_label("beta").click();
    h.run();
    assert_eq!(h.state().ui.row_key.as_deref(), Some("node:beta"));
}

#[test]
fn hover_exposes_peer_counts_and_ping_age_without_animation() {
    let registry = actions::registry();
    let strings = strings();
    let engine = fixture();
    let mut h = Harness::builder()
        .with_context_setup(|ctx| toolkit::install(ctx, &Theme::embedded()))
        .with_size(egui::vec2(1100.0, 900.0))
        .build_ui(move |ui| {
            let _ = view(ui, &engine, &registry, &strings);
        });
    h.run();
    let at = h.get_by_label("gamma").rect().center();
    h.hover_at(at);
    h.run();
    assert!(
        h.query_by_label(&workshop::label_with("mesh-ping-failed", &[("age", "5")]))
            .is_some()
    );
    assert!(h.query_by_label(&label("mesh-since-start")).is_some());
    h.hover_at(egui::pos2(0.0, 0.0));
    h.get_by_label("gamma").focus();
    h.run();
    assert!(h.get_by_label("gamma").is_focused());
    assert!(
        h.query_by_label(&workshop::label_with("mesh-ping-failed", &[("age", "5")]))
            .is_some()
    );
}
