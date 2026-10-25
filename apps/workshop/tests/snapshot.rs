// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Offscreen snapshots of Workshop driven through its real engine: a
//! discovered node with one service whose descriptions failed, a selected
//! command, a body and a completed reply. Regenerate with `UPDATE_SNAPSHOTS=1`
//! and look at the images before committing them.
use design::{DesignContext, Mode, Scheme};
use egui_kittest::Harness;
use inspector::bus::Reply;
use inspector::model::Command;
use inspector::{Dialog, Effect, Engine, Selection, Snapshot};
use toolkit::Theme;
use workshop::{actions, label, strings, view::view};

fn fixture() -> Snapshot {
    let command = |name: &str, args: &str, description: &str, read_only| Command {
        name: name.into(),
        args: args.into(),
        description: description.into(),
        read_only,
    };
    let mut s = Snapshot::default();
    s.services.insert(
        "noded".into(),
        Ok(vec![
            command(
                "noded.list",
                "",
                "Registered services on this node",
                Some(true),
            ),
            command("noded.peers", "", "Mesh peers and routing view", Some(true)),
        ]),
    );
    s.services.insert(
        "settingsd".into(),
        Ok(vec![command(
            "settings.get",
            "{\"key\":\"string\"}",
            "Read one setting",
            Some(true),
        )]),
    );
    s.services.insert(
        "legacy".into(),
        Err("HELP and app.describe unavailable".into()),
    );
    s.peers = vec!["alpha".into(), "beta".into()];
    s.nodes.push(inspector::model::Node {
        name: "local".into(),
        local: true,
        services: s.services.keys().cloned().collect(),
        ..Default::default()
    });
    for name in &s.peers {
        s.nodes.push(inspector::model::Node {
            name: name.clone(),
            ..Default::default()
        });
    }
    let mut identity = bus::ServiceInfo::from_name("settingsd");
    identity.version = Some("0.1.2".into());
    identity.git_sha = Some("123456789abcdef".into());
    s.identities.insert("settingsd".into(), identity);
    s
}

fn engine() -> Engine {
    let mut e = Engine::new(label);
    let Some(Effect::Discover { ticket }) = e.take_effects().pop() else {
        panic!("initial discovery")
    };
    e.discovered(ticket, fixture());
    e.ui.selected = Some(Selection {
        service: "settingsd".into(),
        command: "settings.get".into(),
    });
    e.ui.row_key = Some("command:local:settingsd:settings.get".into());
    e.toggle("mesh");
    e.set_body("{\"key\": \"appearance.scheme\"}".into());
    e.call_selected();
    let Some(Effect::Call { ticket, .. }) = e.take_effects().pop() else {
        panic!("call")
    };
    e.completed(
        ticket,
        Ok(Reply {
            rc: 0,
            body: "{\"key\":\"appearance.scheme\",\"value\":\"ocean\"}".into(),
            error: None,
        }),
    );
    e.take_effects();
    e.workshop.catalogues.insert("global".into(), serde_json::json!({"macros":[{
        "id":"mesh-ping","title":"Mesh ping","tier":"user-global","path":"/home/user/.config/mixos/macros/mesh-ping.mix","shadows":[],"targets":"any"
    }]}));
    e.workshop.catalogues.insert("ced".into(), serde_json::json!({"running":false,"macros":[{
        "id":"tidy-imports","title":"Tidy imports","tier":"user-app","path":"/home/user/.config/mixos/macros/ced/tidy-imports.mix","shadows":["builtin-app"]
    }]}));
    e.ui.expanded.insert("macros:global".into());
    e.ui.expanded.insert("macros:ced".into());
    e.ui.expanded.insert("service:local:ced".into());
    e.workshop.observe = "redacted".into();
    e.workshop.retain(serde_json::json!({"ts":"14:02:11.204","direction":"local","from":"workshop","service":"ced","command":"ced.macros.run","rc":0,"ms":3}));
    e.workshop.retain(serde_json::json!({"ts":"14:02:11.310","direction":"topic","from":"ced","service":"ced","command":"ced.action.executed","rc":0,"ms":1,"payload":{"id":"edit.format","origin":"macro"}}));
    e
}

fn window(theme: Theme, name: &str) {
    window_with(theme, name, None);
}

/// The window, optionally with `dialog` open.
fn window_with(theme: Theme, name: &str, dialog: Option<Dialog>) {
    let mut engine = engine();
    let size = if name == "traffic_right" {
        egui::vec2(1920.0, 1080.0)
    } else {
        egui::vec2(1100.0, 760.0)
    };
    if name.starts_with("macro_") {
        engine.ui.selected = None;
        engine.workshop.selected = Some(("global".into(), "mesh-ping".into()));
        engine.ui.row_key = Some("macro:global:mesh-ping".into());
        engine.workshop.document = serde_json::json!({"tier":"user-global","path":"/home/user/.config/mixos/macros/mesh-ping.mix","text":"-- title: Mesh ping\n-- target: any\nsend noded noded.ping\nprint($result)\n"});
        engine.workshop.message = "run 4 · exited 0".into();
    }
    engine.ui.dialog = dialog;
    let registry = actions::registry();
    let strings = strings();
    let mut harness = Harness::builder()
        .with_size(size)
        .wgpu()
        .build_ui(move |ui| {
            let _ = view(ui, &engine, &registry, &strings);
        });
    toolkit::install(&harness.ctx, &theme);
    harness.run();
    harness.snapshot(name);
}

#[test]
fn traffic_right() {
    window(chrome(Scheme::Studio, Mode::Dark), "traffic_right");
}

#[test]
fn macro_studio_dark() {
    window(chrome(Scheme::Studio, Mode::Dark), "macro_studio_dark");
}

#[test]
fn macro_pro_light() {
    window(chrome(Scheme::Pro, Mode::Light), "macro_pro_light");
}

#[test]
fn macro_breeze_light() {
    window(chrome(Scheme::Breeze, Mode::Light), "macro_breeze_light");
}

/// The hue scheme ocean, light.
#[test]
fn window_light() {
    window(
        Theme::for_context(DesignContext::revision_one()),
        "window_light",
    );
}

/// With no theme chosen Workshop starts in Studio Dark, the look
/// [`window_studio_dark`] pins.
#[test]
fn the_default_theme_is_studio_dark() {
    assert_eq!(Theme::embedded(), chrome(Scheme::Studio, Mode::Dark));
}

/// The chrome scheme pro in its light (medium grey) mode: title bar and
/// menus from the chrome family.
#[test]
fn window_pro_light() {
    window(
        Theme::for_context(DesignContext {
            scheme: Scheme::Pro,
            mode: Mode::Light,
            ..DesignContext::default()
        }),
        "window_pro_light",
    );
}

fn chrome(scheme: Scheme, mode: Mode) -> Theme {
    Theme::for_context(DesignContext {
        scheme,
        mode,
        ..DesignContext::default()
    })
}

/// The chrome scheme studio, dark: cards and pills.
#[test]
fn window_studio_dark() {
    window(chrome(Scheme::Studio, Mode::Dark), "window_studio_dark");
}

#[test]
fn window_larger_text() {
    window(
        Theme::embedded().with_ui(settings::model::CommonUi {
            text_scale: 1.5,
            ..Default::default()
        }),
        "window_larger_text",
    );
}

#[test]
fn window_compact() {
    window(
        Theme::embedded().with_ui(settings::model::CommonUi {
            density: toolkit::ui_scale::DENSITY_STEPS[0],
            ..Default::default()
        }),
        "window_compact",
    );
}

#[test]
fn window_spacious() {
    window(
        Theme::embedded().with_ui(settings::model::CommonUi {
            density: toolkit::ui_scale::DENSITY_STEPS[2],
            ..Default::default()
        }),
        "window_spacious",
    );
}

/// The chrome scheme classic: bevels, square corners.
#[test]
fn window_classic() {
    window(chrome(Scheme::Classic, Mode::Light), "window_classic");
}

#[test]
fn window_breeze_light() {
    window(chrome(Scheme::Breeze, Mode::Light), "window_breeze_light");
}

#[test]
fn window_breeze_dark() {
    window(chrome(Scheme::Breeze, Mode::Dark), "window_breeze_dark");
}

#[test]
fn window_adwaita_light() {
    window(chrome(Scheme::Adwaita, Mode::Light), "window_adwaita_light");
}

#[test]
fn window_adwaita_dark() {
    window(chrome(Scheme::Adwaita, Mode::Dark), "window_adwaita_dark");
}

/// About as a modal dialog over the undimmed window.
#[test]
fn about_pro_light() {
    window_with(
        chrome(Scheme::Pro, Mode::Light),
        "about_pro_light",
        Some(Dialog::About),
    );
}

/// Keyboard shortcuts as a modal dialog.
#[test]
fn shortcuts_studio_light() {
    window_with(
        chrome(Scheme::Studio, Mode::Light),
        "shortcuts_studio_light",
        Some(Dialog::Shortcuts),
    );
}

#[test]
fn window_dark() {
    window(
        Theme::for_context(DesignContext {
            mode: Mode::Dark,
            ..DesignContext::revision_one()
        }),
        "window_dark",
    );
}

fn crossed(scheme: Scheme, style: design::Style, mode: Mode) -> Theme {
    Theme::for_context(DesignContext {
        scheme,
        mode,
        style: Some(style),
        ..DesignContext::default()
    })
}

/// A hue palette in a chrome style: Forest's colours, derived into the
/// chrome roles, in Studio's cards and pills.
#[test]
fn window_forest_studio_dark() {
    window(
        crossed(Scheme::Forest, design::Style::Studio, Mode::Dark),
        "window_forest_studio_dark",
    );
}

/// Ocean's colours in Pro's flat tab strips.
#[test]
fn window_ocean_pro_light() {
    window(
        crossed(Scheme::Ocean, design::Style::Pro, Mode::Light),
        "window_ocean_pro_light",
    );
}

/// A chrome palette in the plain style: Studio's colours through its pairs.
#[test]
fn window_studio_plain_dark() {
    window(
        crossed(Scheme::Studio, design::Style::Plain, Mode::Dark),
        "window_studio_plain_dark",
    );
}

/// Server-side decorations: no title, captions or mark; the menus and the
/// right-hand controls in a menu-bar row.
#[test]
fn window_ssd() {
    let theme = Theme::embedded().framed(design::Decorations::Server, design::CaptionSide::Right);
    window(theme, "window_ssd");
}

/// The captions at the left, Close in the top-left corner.
#[test]
fn window_captions_left() {
    let theme = Theme::embedded().framed(design::Decorations::Client, design::CaptionSide::Left);
    window(theme, "window_captions_left");
}
