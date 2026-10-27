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
    engine
        .workshop
        .mesh
        .set_probe(&json!({"gamma":{"ok":false,"rc":10,"at":95_000}}), clock());
    engine
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
