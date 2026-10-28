// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Offscreen snapshots of Preferences driven through its real engine: the
//! Applications editor with a listed catalogue (an update, a current app, a
//! new one and a failed check), a selected app with its release notes, and
//! the remove confirmation; the Appearance editor as read and with an
//! unapplied draft the window previews; Apps → Email with no
//! accounts, two accounts with one selected, and a refused new account.
//! Regenerate with `UPDATE_SNAPSHOTS=1` and look at
//! the images before committing them.
use citizen::Reply;
use design::{CaptionSide, DesignContext, Mode, Scheme, Style};
use egui_kittest::Harness;
use preferences::email::Field;
use preferences::{Dialog, Effect, Engine, Look, Section, SoftwareGroup};
use prefs::{actions, label_with, strings, view::view};
use serde_json::json;
use toolkit::Theme;

/// A `settings.get` reply in the summary view Preferences reads.
fn summary_reply(revision: &str, appearance: serde_json::Value) -> serde_json::Value {
    json!({"status":"current","summary":{"schema":1,
        "binding":{"instance":"example","profile":"default"},
        "incarnation":"inc-1","revision":revision,"design_revision":revision,
        "source_digest":"d","appearance":appearance,"custom_source":false}})
}

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

#[test]
fn text_and_fonts() {
    let mut e = appearance_engine();
    e.set_section(Section::TextAndFonts);
    draw_at_size(
        chrome(Scheme::Studio, Mode::Dark),
        "text_and_fonts",
        e,
        egui::vec2(980.0, 960.0),
    );
}

#[test]
fn interface_font_selected() {
    let mut e = appearance_engine();
    e.set_section(Section::TextAndFonts);
    e.look.interface_font = Some(preferences::appearance::InterfaceFont {
        weight: Some(700),
        size: Some(18.0),
    });
    let theme = chrome(Scheme::Studio, Mode::Dark)
        .with_overlay(toolkit::theme::Overlay {
            weight: Some(700),
            size: Some(18.0),
            ..Default::default()
        })
        .unwrap();
    draw_at_size(
        theme,
        "interface_font_selected",
        e,
        egui::vec2(1100.0, 1100.0),
    );
}

#[test]
fn interface_font_refusal() {
    let mut e = appearance_engine();
    e.set_section(Section::TextAndFonts);
    e.edit_interface_font(preferences::text::InterfaceEdit::Size(Some(16.0)));
    let ticket = e
        .take_effects()
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Settings { ticket, .. } => Some(ticket),
            _ => None,
        })
        .expect("the font apply");
    e.settled(
        ticket,
        Ok(Reply {
            rc: 10,
            body: json!({"status":"validation_failed",
                "diagnostics":[{"path":"appearance.font_size", "message":"Unknown field or scope"}]
            })
            .to_string(),
            error: None,
        }),
    );
    assert!(
        e.look
            .problem
            .as_ref()
            .unwrap()
            .contains("Unknown field or scope")
    );
    draw_at_size(
        chrome(Scheme::Studio, Mode::Dark),
        "interface_font_refusal",
        e,
        egui::vec2(980.0, 960.0),
    );
}

#[test]
fn windows() {
    let mut e = appearance_engine();
    e.set_section(Section::Windows);
    draw(chrome(Scheme::Studio, Mode::Dark), "windows", e);
}

#[test]
fn accessibility() {
    let mut e = appearance_engine();
    e.set_section(Section::Accessibility);
    draw(chrome(Scheme::Studio, Mode::Dark), "accessibility", e);
}

#[test]
fn apps_term() {
    let mut e = appearance_engine();
    e.set_section(Section::Terminal);
    e.look.terminal = Some(Default::default());
    draw_at_size(
        chrome(Scheme::Studio, Mode::Dark),
        "apps_term",
        e,
        egui::vec2(980.0, 960.0),
    );
}

#[test]
fn apps_ced() {
    let mut e = appearance_engine();
    e.set_section(Section::Ced);
    e.look.email = Some(Default::default());
    e.look.ced = Some(Default::default());
    draw_at_size(
        chrome(Scheme::Studio, Mode::Dark),
        "apps_ced",
        e,
        egui::vec2(980.0, 960.0),
    );
}

#[test]
fn creative_studio_dark() {
    let mut e = Engine::new(label_with);
    answer(
        &mut e,
        json!([
            {"app":"canvas","repo":"example/canvas","category":"creative","installed":"0.8.0","latest":"0.9.0","published":"2026-10-10","status":"update"},
            {"app":"drawing","repo":"example/drawing","category":"creative","installed":null,"latest":"0.9.0","published":"2026-10-10","status":"not installed"}
        ]),
    );
    e.set_section(Section::Software);
    e.set_software_group(SoftwareGroup::Creative);
    draw(
        chrome(Scheme::Studio, Mode::Dark),
        "creative_studio_dark",
        e,
    );
}

#[test]
fn desktop_studio_light() {
    let mut e = engine();
    e.set_software_group(SoftwareGroup::Desktop);
    draw(
        chrome(Scheme::Studio, Mode::Light),
        "desktop_studio_light",
        e,
    );
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
    let snapshot = summary_reply(
        "12",
        json!({"scheme":"studio","mode":"dark","contrast":"normal","source":null}),
    );
    e.settled(
        ticket,
        Ok(Reply {
            rc: 0,
            body: snapshot.to_string(),
            error: None,
        }),
    );
    e.set_section(Section::Appearance);
    wallpaper_read(&mut e, json!({"schema":1}));
    e
}

fn wallpaper_read(e: &mut Engine, config: serde_json::Value) {
    e.read_wallpaper();
    let ticket = e
        .take_effects()
        .into_iter()
        .find_map(|f| match f {
            Effect::Backdrop { ticket, .. } => Some(ticket),
            _ => None,
        })
        .unwrap();
    e.wallpaper_finished(ticket,Ok(Reply{rc:0,body:json!({"schema":"backdrop.v1","config":config,"pending":null,"outputs":[{"name":"DP-1"}]}).to_string(),error:None}));
}

#[test]
fn wallpaper_colour_studio_dark() {
    let mut e = appearance_engine();
    e.set_section(Section::Wallpapers);
    e.wallpaper_kind(preferences::wallpaper::Kind::Colour);
    e.wallpaper_colour("#25354a".into());
    draw(
        chrome(Scheme::Studio, Mode::Dark),
        "wallpaper_colour_studio_dark",
        e,
    );
}

#[test]
fn wallpaper_image_pro_light() {
    let mut e = appearance_engine();
    e.set_section(Section::Wallpapers);
    wallpaper_read(
        &mut e,
        json!({"schema":1,"default":{"image":"/usr/share/backgrounds/landscape.jpg"}}),
    );
    draw(
        chrome(Scheme::Pro, Mode::Light),
        "wallpaper_image_pro_light",
        e,
    );
}

#[test]
fn wallpaper_refused_studio_dark() {
    let mut e = appearance_engine();
    e.set_section(Section::Wallpapers);
    e.wallpaper_kind(preferences::wallpaper::Kind::Image);
    e.wallpaper_image("/pictures/missing.png".into());
    e.wallpaper.status = "status.refused".into();
    e.wallpaper.failed = true;
    e.wallpaper.detail = Some("Image could not be opened: No such file or directory".into());
    draw(
        chrome(Scheme::Studio, Mode::Dark),
        "wallpaper_refused_studio_dark",
        e,
    );
}

#[test]
fn wallpaper_picker_studio_dark() {
    let mut e = appearance_engine();
    e.set_section(Section::Wallpapers);
    e.picker.open = true;
    e.picker.directory = "/usr/share/backgrounds".into();
    e.picker.entries = vec![
        preferences::picker::Entry {
            name: "landscapes".into(),
            path: "/usr/share/backgrounds/landscapes".into(),
            directory: true,
        },
        preferences::picker::Entry {
            name: "mountains.jpg".into(),
            path: "/usr/share/backgrounds/mountains.jpg".into(),
            directory: false,
        },
    ];
    e.picker.selected = Some("/usr/share/backgrounds/mountains.jpg".into());
    draw(
        chrome(Scheme::Studio, Mode::Dark),
        "wallpaper_picker_studio_dark",
        e,
    );
}

fn draw(theme: Theme, name: &str, engine: Engine) {
    draw_at_size(theme, name, engine, egui::vec2(980.0, 640.0));
}

fn draw_at_size(theme: Theme, name: &str, engine: Engine, size: egui::Vec2) {
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
fn minimum_window_renders() {
    for section in [
        Section::Appearance,
        Section::Software,
        Section::Email,
        Section::Terminal,
        Section::Ced,
        Section::TextAndFonts,
        Section::Windows,
        Section::Accessibility,
        Section::Wallpapers,
    ] {
        let mut engine = appearance_engine();
        engine.set_section(section);
        let registry = actions::registry();
        let strings = strings();
        let mut h = Harness::builder()
            .with_context_setup(|ctx| toolkit::install(ctx, &Theme::embedded()))
            .with_size(egui::Vec2::from(toolkit::window::MIN_SIZE))
            .build_ui(move |ui| {
                let _ = view(ui, &engine, &registry, &strings);
            });
        toolkit::install(&h.ctx, &Theme::embedded());
        h.run_steps(4);
        h.snapshot(format!("minimum_window_{}", section.name()));
    }
}

/// The default look: Studio Dark.
#[test]
fn software() {
    window_with(chrome(Scheme::Studio, Mode::Dark), "software", None);
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
fn appearance() {
    draw(
        chrome(Scheme::Studio, Mode::Dark),
        "appearance",
        appearance_engine(),
    );
}

/// A change being applied (Forest, Pro style, light, captions left): it
/// applies at once, so the window and the panel show it straight away, with
/// no Apply or Revert to press.
#[test]
fn appearance_applying_forest_pro_light() {
    let mut engine = appearance_engine();
    let draft = Look {
        scheme: Scheme::Forest,
        style: Some(Style::Pro),
        mode: Mode::Light,
        captions: "left".into(),
        ..engine.look.current.clone().expect("read")
    };
    engine.edit_look(draft.clone());
    assert_eq!(engine.look.shown(), Some(draft.clone()));
    // As the shell installs it: the expected look over the embedded design.
    let theme = Theme::for_context(draft.context())
        .framed(draft.decorations, CaptionSide::resolve(&draft.captions));
    draw(theme, "appearance_applying_forest_pro_light", engine);
}

/// The scheme was changed to Forest while another writer changed it to
/// Ocean: the panel names the conflict and offers Keep my changes and
/// Revert; nothing applies until one is chosen.
#[test]
fn appearance_conflict_studio_dark() {
    let mut engine = appearance_engine();
    let mut draft = engine.look.current.clone().expect("read");
    draft.scheme = Scheme::Forest;
    engine.edit_look(draft);
    // Another writer was first: the apply is refused with a conflict, and
    // the read shows the scheme changed to Ocean.
    let settings_ticket = |engine: &mut Engine| {
        engine
            .take_effects()
            .into_iter()
            .find_map(|x| match x {
                Effect::Settings { ticket, .. } => Some(ticket),
                _ => None,
            })
            .expect("a settings call")
    };
    let apply = settings_ticket(&mut engine);
    engine.settled(
        apply,
        Ok(Reply {
            rc: 10,
            body: json!({"status":"conflict","incarnation":"inc-1","revision":"13"}).to_string(),
            error: None,
        }),
    );
    let ticket = settings_ticket(&mut engine);
    let ocean = summary_reply(
        "13",
        json!({"scheme":"ocean","mode":"dark","contrast":"normal","source":null}),
    );
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
    let theme = Theme::for_context(look.context())
        .framed(look.decorations, CaptionSide::resolve(&look.captions));
    draw(theme, "appearance_conflict_studio_dark", engine);
}

/// The Email section over a settingsd read holding `email`, with Email's
/// `email.accounts` answered by `passwords` (or still unasked).
fn email_engine(email: serde_json::Value, passwords: Option<serde_json::Value>) -> Engine {
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
    let mut snapshot = summary_reply(
        "12",
        json!({"scheme":"studio","mode":"dark","contrast":"normal","source":null}),
    );
    snapshot["summary"]["email"] = email;
    e.settled(
        ticket,
        Ok(Reply {
            rc: 0,
            body: snapshot.to_string(),
            error: None,
        }),
    );
    e.set_section(Section::Email);
    let ticket = e
        .take_effects()
        .into_iter()
        .find_map(|x| match x {
            Effect::Mail { ticket, .. } => Some(ticket),
            _ => None,
        })
        .expect("Email asked about passwords");
    if let Some(passwords) = passwords {
        e.mailed(
            ticket,
            Ok(Reply {
                rc: 0,
                body: passwords.to_string(),
                error: None,
            }),
        );
    }
    e
}

/// No accounts yet: the list says so and Add account is the way in.
#[test]
fn apps_email() {
    let e = email_engine(json!({}), None);
    draw_at_size(
        chrome(Scheme::Studio, Mode::Dark),
        "apps_email",
        e,
        egui::vec2(980.0, 1050.0),
    );
}

/// Two accounts, Work the default and Home disabled; Home selected, its
/// password set in Email.
#[test]
fn email_accounts_studio_dark() {
    let mut e = email_engine(
        json!({"accounts":[
            {"id":"acct-0f3a","name":"Work","url":"https://mail.example.com","email":"admin@example.com"},
            {"id":"acct-9b21","name":"Home","url":"https://jmap.example.net","email":"mark@example.net",
             "enabled":false,"signature":"-- \nMark"}
        ],"default_account":"acct-0f3a","mail":{"page_size":50}}),
        Some(
            json!({"accounts":[{"id":"acct-0f3a","has_password":true},{"id":"acct-9b21","has_password":true}]}),
        ),
    );
    e.select_email_account("acct-9b21");
    draw_at_size(
        chrome(Scheme::Studio, Mode::Dark),
        "email_accounts_studio_dark",
        e,
        egui::vec2(980.0, 900.0),
    );
}

#[test]
fn email_rejected_password_standard_size() {
    let mut e = email_engine(
        json!({"accounts":[
            {"id":"acct-example","name":"Work","url":"https://mail.example.com","email":"admin@example.com"}
        ]}),
        Some(json!({"accounts":[{"id":"acct-example","has_password":false}]})),
    );
    e.select_email_account("acct-example");
    e.email.outcome = Some(("Password rejected by Email".into(), true));
    assert!(e.look.problem.is_none());
    draw(
        chrome(Scheme::Studio, Mode::Dark),
        "email_rejected_password_standard_size",
        e,
    );
}

/// A new account whose Save was refused before sending: the URL is not
/// https and the address has no @; each field says why.
#[test]
fn email_invalid_pro_light() {
    let mut e = email_engine(
        json!({"accounts":[
            {"id":"acct-0f3a","name":"Work","url":"https://mail.example.com","email":"admin@example.com"}
        ]}),
        None,
    );
    e.new_email_account();
    e.edit_email_field(Field::Name, "Gold Coast".into());
    e.edit_email_field(Field::Url, "http://mail.example.com/".into());
    e.edit_email_field(Field::Email, "admin".into());
    e.save_email_account();
    assert!(e.failed, "{}", e.status);
    assert_eq!(e.email.form.as_ref().map(|f| f.errors.len()), Some(2));
    draw(
        chrome(Scheme::Pro, Mode::Light),
        "email_invalid_pro_light",
        e,
    );
}

/// The releases service is not on the Bus: the panel says so.
#[test]
fn releases_missing_studio_dark() {
    let mut engine = Engine::new(label_with);
    engine.set_section(Section::Software);
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
    let registry = actions::registry();
    let strings = strings();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(980.0, 640.0))
        .wgpu()
        .build_ui(move |ui| {
            let _ = view(ui, &engine, &registry, &strings);
        });
    toolkit::install(&harness.ctx, &theme);
    harness.run();
    harness.snapshot("releases_missing_studio_dark");
}
