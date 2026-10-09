// SPDX-License-Identifier: MIT OR Apache-2.0
//! Offscreen snapshots of BusViewer driven through its real engine: a
//! discovered node with one service whose descriptions failed, a selected
//! verb, a body and a completed reply. Regenerate with `UPDATE_SNAPSHOTS=1`
//! and look at the images before committing them.
use busviewer::{commands, label, strings, view::view};
use design::{DesignContext, Mode, Scheme};
use egui_kittest::Harness;
use inspector::bus::Reply;
use inspector::model::Verb;
use inspector::{Dialog, Effect, Engine, Selection, Snapshot};
use toolkit::{Theme, icons};

fn fixture() -> Snapshot {
    let verb = |name: &str, args: &str, description: &str, read_only| Verb {
        name: name.into(),
        args: args.into(),
        description: description.into(),
        read_only,
    };
    let mut s = Snapshot::default();
    s.services.insert(
        "noded".into(),
        Ok(vec![
            verb("noded.list", "", "Registered services on this node", Some(true)),
            verb("noded.peers", "", "Mesh peers and routing view", Some(true)),
        ]),
    );
    s.services.insert(
        "settingsd".into(),
        Ok(vec![verb("settings.get", "{\"key\":\"string\"}", "Read one setting", Some(true))]),
    );
    s.services.insert("legacy".into(), Err("HELP and app.describe unavailable".into()));
    s.peers = vec!["alpha".into(), "beta".into()];
    s
}

fn engine() -> Engine {
    let mut e = Engine::new(label);
    let Some(Effect::Discover { ticket }) = e.take_effects().pop() else { panic!("initial discovery") };
    e.discovered(ticket, fixture());
    e.ui.selected = Some(Selection { service: "settingsd".into(), verb: "settings.get".into() });
    e.ui.row_key = Some("verb:settingsd:settings.get".into());
    e.toggle("mesh");
    e.set_body("{\"key\": \"appearance.scheme\"}".into());
    e.call_selected();
    let Some(Effect::Call { ticket, .. }) = e.take_effects().pop() else { panic!("call") };
    e.completed(ticket, Ok(Reply { rc: 0, body: "{\"key\":\"appearance.scheme\",\"value\":\"ocean\"}".into() }));
    e.take_effects();
    e
}

fn window(theme: Theme, name: &str) {
    window_with(theme, name, None);
}

/// The window, optionally with `dialog` open.
fn window_with(theme: Theme, name: &str, dialog: Option<Dialog>) {
    let mut engine = engine();
    engine.ui.dialog = dialog;
    let registry = commands::registry();
    let strings = strings();
    let stroke = icons::stroke_width(&theme);
    let mut harness = Harness::builder().with_size(egui::vec2(980.0, 620.0)).wgpu().build_ui(move |ui| {
        let _ = view(ui, &engine, &registry, &strings, stroke);
    });
    toolkit::install(&harness.ctx, &theme);
    harness.run();
    harness.snapshot(name);
}

#[test]
fn window_light() {
    window(Theme::embedded(), "window_light");
}

/// The chrome scheme pro in its light (medium grey) mode: title bar and
/// menus from the chrome family.
#[test]
fn window_pro_light() {
    window(Theme::for_context(DesignContext { scheme: Scheme::Pro, mode: Mode::Light, ..DesignContext::default() }), "window_pro_light");
}

fn chrome(scheme: Scheme, mode: Mode) -> Theme {
    Theme::for_context(DesignContext { scheme, mode, ..DesignContext::default() })
}

/// The chrome scheme studio, dark: cards and pills.
#[test]
fn window_studio_dark() {
    window(chrome(Scheme::Studio, Mode::Dark), "window_studio_dark");
}

/// The chrome scheme classic: bevels, square corners.
#[test]
fn window_classic() {
    window(chrome(Scheme::Classic, Mode::Light), "window_classic");
}

/// About as a modal dialog over the undimmed window.
#[test]
fn about_pro_light() {
    window_with(chrome(Scheme::Pro, Mode::Light), "about_pro_light", Some(Dialog::About));
}

/// Keyboard shortcuts as a modal dialog.
#[test]
fn shortcuts_studio_light() {
    window_with(chrome(Scheme::Studio, Mode::Light), "shortcuts_studio_light", Some(Dialog::Shortcuts));
}

#[test]
fn window_dark() {
    window(Theme::for_context(DesignContext { mode: Mode::Dark, ..DesignContext::default() }), "window_dark");
}
