// SPDX-License-Identifier: MIT OR Apache-2.0
//! Offscreen snapshots of Prefs driven through its real engine: the
//! Applications editor with a listed catalogue (an update, a current app, a
//! new one and a failed check), a selected app with its release notes, and
//! the remove confirmation; the Appearance editor as read and with an
//! unapplied draft the window previews. Regenerate with `UPDATE_SNAPSHOTS=1` and look at
//! the images before committing them.
use citizen::Reply;
use design::{CaptionSide, DesignContext, Mode, Scheme, Style};
use egui_kittest::Harness;
use preferences::{Dialog, Effect, Engine, Look, Panel};
use prefs::{commands, label_with, strings, view::view};
use serde_json::json;
use toolkit::{Theme, icons};

fn answer(e: &mut Engine, body: serde_json::Value) {
    let ticket = e
        .take_effects()
        .into_iter()
        .find_map(|x| match x {
            Effect::Releases { ticket, .. } => Some(ticket),
            _ => None,
        })
        .expect("a releases call");
    e.released(
        ticket,
        Ok(Reply {
            rc: 0,
            body: body.to_string(),
            error: None,
        }),
    );
}

fn engine() -> Engine {
    let mut e = Engine::new(label_with);
    answer(
        &mut e,
        json!([
            {"app":"atlas","repo":"example/atlas","installed":"0.8.0","latest":"0.9.0","published":"2026-10-10","status":"update"},
            {"app":"loom","repo":"example/loom","installed":"1.2.0","latest":"1.2.0","published":"2026-10-08","status":"current"},
            {"app":"quill","repo":"example/quill","installed":null,"latest":"0.4.1","published":"2026-10-09","status":"not installed"},
            {"app":"tern","repo":"example/tern","installed":null,"latest":null,"published":null,"status":"error: GitHub answered 403"}
        ]),
    );
    e.select("atlas");
    answer(
        &mut e,
        json!({"app":"atlas","tag":"v0.9.0","published":"2026-10-10T02:51:48Z","url":"https://example.org/atlas/releases/v0.9.0",
            "notes":"## What's changed\n* Faster tile rendering on large canvases\n* Fix the export dialog losing its folder\n* New: snap to guides"}),
    );
    e
}

fn chrome(scheme: Scheme, mode: Mode) -> Theme {
    Theme::for_context(DesignContext {
        scheme,
        mode,
        ..DesignContext::default()
    })
}

/// The window, optionally with `dialog` open.
fn window_with(theme: Theme, name: &str, dialog: Option<Dialog>) {
    let mut engine = engine();
    engine.ui.dialog = dialog;
    draw(theme, name, engine);
}

/// The Appearance panel with settingsd's look read (Studio, the scheme's
/// own style, dark).
fn appearance_engine() -> Engine {
    let mut e = engine();
    e.start_appearance("example");
    let ticket = e
        .take_effects()
        .into_iter()
        .find_map(|x| match x {
            Effect::Settings { ticket, .. } => Some(ticket),
            _ => None,
        })
        .expect("a settings read");
    let snapshot = json!({"status":"current","snapshot":{"incarnation":"inc-1","revision":"12",
        "desktop":{"appearance":{"scheme":"studio","mode":"dark","contrast":"normal","source":null}}}});
    e.settled(
        ticket,
        Ok(Reply {
            rc: 0,
            body: snapshot.to_string(),
            error: None,
        }),
    );
    e.ui.panel = Panel::Appearance;
    e
}

fn draw(theme: Theme, name: &str, engine: Engine) {
    let registry = commands::registry();
    let strings = strings();
    let stroke = icons::stroke_width(&theme);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(980.0, 640.0))
        .wgpu()
        .build_ui(move |ui| {
            let _ = view(ui, &engine, &registry, &strings, stroke);
        });
    toolkit::install(&harness.ctx, &theme);
    harness.run();
    harness.snapshot(name);
}

/// The default look: Studio Dark.
#[test]
fn applications_studio_dark() {
    window_with(
        chrome(Scheme::Studio, Mode::Dark),
        "applications_studio_dark",
        None,
    );
}

/// The Pro chrome in its light mode.
#[test]
fn applications_pro_light() {
    window_with(
        chrome(Scheme::Pro, Mode::Light),
        "applications_pro_light",
        None,
    );
}

/// Remove asks first; Cancel is the default button.
#[test]
fn remove_confirmation_studio_light() {
    window_with(
        chrome(Scheme::Studio, Mode::Light),
        "remove_studio_light",
        Some(Dialog::Remove("atlas".into())),
    );
}

/// Appearance as settingsd holds it, nothing edited.
#[test]
fn appearance_studio_dark() {
    draw(
        chrome(Scheme::Studio, Mode::Dark),
        "appearance_studio_dark",
        appearance_engine(),
    );
}

/// An unapplied draft (Forest, Pro style, light, captions left): the window
/// previews it, Apply and Revert are enabled.
#[test]
fn appearance_draft_forest_pro_light() {
    let mut engine = appearance_engine();
    let draft = Look {
        scheme: Scheme::Forest,
        style: Some(Style::Pro),
        mode: Mode::Light,
        captions: CaptionSide::Left,
        ..engine.look.current.expect("read")
    };
    engine.edit_look(draft);
    // As the shell installs it: the draft over the embedded design.
    let theme = Theme::for_context(draft.context()).framed(draft.decorations, draft.captions);
    draw(theme, "appearance_draft_forest_pro_light", engine);
}

/// The scheme was edited to Forest while another writer changed it to
/// Ocean: the panel names the conflict, offers Keep my changes, and Apply
/// waits.
#[test]
fn appearance_conflict_studio_dark() {
    let mut engine = appearance_engine();
    let mut draft = engine.look.current.expect("read");
    draft.scheme = Scheme::Forest;
    engine.edit_look(draft);
    engine.read_look();
    let ticket = engine
        .take_effects()
        .into_iter()
        .find_map(|x| match x {
            Effect::Settings { ticket, .. } => Some(ticket),
            _ => None,
        })
        .expect("a re-read");
    let ocean = json!({"status":"current","snapshot":{"incarnation":"inc-1","revision":"13",
        "desktop":{"appearance":{"scheme":"ocean","mode":"dark","contrast":"normal","source":null}}}});
    engine.settled(
        ticket,
        Ok(Reply {
            rc: 0,
            body: ocean.to_string(),
            error: None,
        }),
    );
    assert!(!engine.look.conflicts.is_empty());
    let look = engine.look.shown().expect("shown");
    let theme = Theme::for_context(look.context()).framed(look.decorations, look.captions);
    draw(theme, "appearance_conflict_studio_dark", engine);
}

/// The releases service is not on the Bus: the panel says so.
#[test]
fn releases_missing_studio_dark() {
    let mut engine = Engine::new(label_with);
    let ticket = engine
        .take_effects()
        .into_iter()
        .find_map(|x| match x {
            Effect::Releases { ticket, .. } => Some(ticket),
            _ => None,
        })
        .unwrap();
    engine.released(
        ticket,
        Ok(Reply {
            rc: 10,
            body: String::new(),
            error: None,
        }),
    );
    let theme = chrome(Scheme::Studio, Mode::Dark);
    let registry = commands::registry();
    let strings = strings();
    let stroke = icons::stroke_width(&theme);
    let mut harness = Harness::builder()
        .with_size(egui::vec2(980.0, 640.0))
        .wgpu()
        .build_ui(move |ui| {
            let _ = view(ui, &engine, &registry, &strings, stroke);
        });
    toolkit::install(&harness.ctx, &theme);
    harness.run();
    harness.snapshot("releases_missing_studio_dark");
}
