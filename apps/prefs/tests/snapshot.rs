// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Offscreen snapshots of the pages at COSMIC Settings' default window,
//! 1024 by 720, with the values the reference desk was measured at (compact density, a grey
//! accent, dark), for the stage 5 side-by-side against cosmic-settings on
//! VT2 (`_var/cleanroom/cosmic-shots/win-*.png`). Also the condensed
//! window, a page landed on from search, a missing backend and a stale
//! mirror. Regenerate with `UPDATE_SNAPSHOTS=1` and look at the images
//! before committing them.
use egui_kittest::Harness;
use preferences::Engine;
use prefs::{actions, cosmic, label_with, strings};
use serde_json::{Value, json};
use toolkit::Theme;

/// The reference desk's live values (spec 4, "Measurement conditions").
fn measured() -> Value {
    json!({"appearance.density":"compact","appearance.accent":"#7f7f7f","appearance.mode":"dark",
        "state.appearance.daylight":true})
}

fn engine(values: Value, role: &str, stale: bool) -> Engine {
    let mut e = Engine::new(label_with);
    e.store_read(&json!({"incarnation":"inc-1","revision":"7","values":values}));
    e.store_status(&json!({"role": role, "stale": stale}));
    e.take_effects();
    e
}

/// The window's theme for the store's values, as the shell derives it.
fn theme(e: &Engine) -> Theme {
    let view = <settings::view::Look as settings::view::View>::of(&e.store.values);
    toolkit::session_look::follow_look(&view, Some("prefs"), false, &Theme::embedded())
}

fn draw(name: &str, engine: Engine, size: egui::Vec2) {
    let registry = actions::registry();
    let strings = strings();
    let theme = theme(&engine);
    let look = cosmic::Look::of(&theme, &engine);
    let mut harness = Harness::builder()
        .with_size(size)
        .wgpu()
        .build_ui(move |ui| {
            let _ = cosmic::view(ui, &engine, &registry, &strings, &look);
        });
    toolkit::install(&harness.ctx, &theme);
    // egui marks a fractional cursor in debug builds; the release window
    // draws nothing there.
    harness
        .ctx
        .global_style_mut(|s| s.debug.show_unaligned = false);
    harness.run_steps(6);
    harness.snapshot(name);
}

const COSMIC: egui::Vec2 = egui::vec2(1024.0, 720.0);

fn page(id: &str, row: Option<&str>) -> Engine {
    let mut e = engine(measured(), "authority", false);
    e.open_page(id, row).unwrap();
    e
}

#[test]
fn appearance() {
    draw("appearance", page("appearance", None), COSMIC);
}

#[test]
fn appearance_density_landing() {
    draw(
        "appearance_density",
        page("appearance", Some("density")),
        COSMIC,
    );
}

#[test]
fn appearance_frosted_drawer() {
    draw(
        "appearance_frosted",
        page("appearance", Some("frosted_thickness")),
        COSMIC,
    );
}

#[test]
fn panel() {
    draw("panel", page("panel", None), COSMIC);
}

#[test]
fn window_management() {
    draw("window_management", page("window_management", None), COSMIC);
}

#[test]
fn dock() {
    draw("dock", page("dock", None), COSMIC);
}

#[test]
fn workspaces() {
    draw("workspaces", page("workspaces", None), COSMIC);
}

#[test]
fn wifi_missing_backend() {
    draw("wifi", page("wifi", None), COSMIC);
}

#[test]
fn date_time() {
    draw("date_time", page("date_time", None), COSMIC);
}

#[test]
fn region() {
    draw("region", page("region", None), COSMIC);
}

#[test]
fn accessibility() {
    draw("accessibility", page("accessibility", None), COSMIC);
}

#[test]
fn magnifier_missing_backend() {
    draw("magnifier", page("magnifier", None), COSMIC);
}

#[test]
fn osk_missing_backend() {
    draw("osk", page("osk", None), COSMIC);
}

#[test]
fn power_missing_backends() {
    draw("power", page("power", None), COSMIC);
}

#[test]
fn desktop_sub_pages() {
    draw("desktop", page("desktop", None), COSMIC);
}

#[test]
fn search_results() {
    let mut e = page("desktop", None);
    e.set_query("hint".into());
    draw("search_hint", e, COSMIC);
}

#[test]
fn stale_mirror() {
    let mut e = engine(measured(), "mirror", true);
    e.open_page("dock", None).unwrap();
    draw("dock_stale_mirror", e, COSMIC);
}

#[test]
fn light_comfortable() {
    let mut e = engine(
        json!({"appearance.mode":"light","state.appearance.daylight":true}),
        "authority",
        false,
    );
    e.open_page("appearance", None).unwrap();
    draw("appearance_light_comfortable", e, COSMIC);
}

#[test]
fn condensed_window() {
    draw("condensed", page("panel", None), egui::vec2(560.0, 640.0));
}

/// Layouts as the xkb rules describe them, fixed so the snapshots do not
/// follow the worker's xkeyboard-config.
fn keyboard(drawer: Option<&str>) -> Engine {
    let mut e = page("keyboard", None);
    e.set_options(
        preferences::xkb::LAYOUTS,
        vec![
            (json!(["us", ""]), "English (US)".into()),
            (json!(["us", "colemak"]), "English (Colemak)".into()),
            (json!(["us", "dvorak"]), "English (Dvorak)".into()),
            (json!(["de", ""]), "German".into()),
            (json!(["de", "nodeadkeys"]), "German (no dead keys)".into()),
        ],
    );
    if let Some(drawer) = drawer {
        e.open_drawer(drawer);
    }
    e
}

#[test]
fn keyboard_page() {
    draw("keyboard", keyboard(None), COSMIC);
}

#[test]
fn keyboard_add_source_drawer() {
    draw("keyboard_add_source", keyboard(Some("add_source")), COSMIC);
}
