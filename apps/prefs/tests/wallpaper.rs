// SPDX-License-Identifier: MIT OR Apache-2.0
//! The real UI event path through the pure editor and its Bus effects.
use citizen::Reply;
use preferences::{Effect, Engine, picker::Entry, wallpaper::COLOUR_PRESETS};
use prefs::{commands, label_with, shell::apply_ui, view::UiEvent};
use serde_json::{Value, json};
use toolkit::command::Origin;

fn reply(rc: u8, body: Value) -> Result<Reply, citizen::CallError> {
    Ok(Reply { rc, body: body.to_string(), error: None })
}

fn state(config: Value) -> Result<Reply, citizen::CallError> {
    reply(0, json!({"schema":"backdrop.v1", "config": config, "pending":null, "outputs":[]}))
}

fn call(e: &mut Engine, expected: &str) -> (u64, Value) {
    e.take_effects().into_iter().find_map(|effect| match effect {
        Effect::Backdrop { ticket, verb, body } if verb == expected => Some((ticket, body)),
        _ => None,
    }).expect("expected backdrop call")
}

fn loaded(config: Value) -> Engine {
    let mut e = Engine::new(label_with);
    e.take_effects();
    e.read_wallpaper();
    let (ticket, _) = call(&mut e, "backdrop.state");
    e.wallpaper_finished(ticket, state(config));
    e
}

fn set_after_preflight(e: &mut Engine, config: Value) -> (u64, Value) {
    let (ticket, _) = call(e, "backdrop.state");
    e.wallpaper_finished(ticket, state(config));
    call(e, "backdrop.set")
}

#[test]
fn choosing_a_preset_sends_the_complete_colour_body() {
    for (key, colour) in COLOUR_PRESETS {
        assert_ne!(prefs::label(key), key);
        let config = json!({"schema":1});
        let mut e = loaded(config.clone());
        // Commands are deliberately first: apply_ui orders edits before them.
        apply_ui(&mut e, &commands::registry(), vec![
            UiEvent::Command("wallpaper.apply", Origin::Ui),
            UiEvent::WallpaperColour(colour.into()),
        ]);
        let (_, body) = set_after_preflight(&mut e, config);
        assert_eq!(body, json!({"schema":1,"default":{"colour":colour}}));
    }
}

#[test]
fn choosing_an_image_sends_the_body_and_refusal_restores_the_displayed_choice() {
    let config = json!({"schema":1,"default":{"image":"/pictures/good.jpg","fallback":"#112233"}});
    let mut e = loaded(config.clone());
    e.picker.open = true;
    e.picker.entries = vec![Entry { name:"bad.png".into(), path:"/pictures/bad.png".into(), directory:false }];
    e.picker_select("/pictures/bad.png");
    commands::registry().execute("wallpaper.picker.use", &mut e, Origin::Ui).unwrap();
    assert!(!e.picker.open);
    let (ticket, body) = set_after_preflight(&mut e, config.clone());
    assert_eq!(body, json!({"schema":1,"default":{"image":"/pictures/bad.png","fallback":"#112233"}}));
    e.wallpaper_finished(ticket, reply(10, json!({"error_code":"IMAGE", "message":"corrupt PNG"})));
    assert_eq!(e.wallpaper.fill(), config["default"]);
    assert_eq!(e.wallpaper.current, Some(config));
    assert!(e.ui.wallpaper_image.is_none());
    assert!(e.ui.wallpaper_kind.is_none());
    assert_eq!(e.wallpaper.detail.as_deref(), Some("corrupt PNG"));
    assert_eq!(e.wallpaper.status, "status.refused");
}

#[test]
fn invalid_hex_sends_no_set_and_acceptance_is_shown_after_the_reread() {
    let config = json!({"schema":1});
    let mut e = loaded(config.clone());
    for colour in ["#fff", "#GG0000", "123456", "#12345678"] {
        apply_ui(&mut e, &commands::registry(), vec![
            UiEvent::WallpaperColour(colour.into()),
            UiEvent::Command("wallpaper.apply", Origin::Ui),
        ]);
        assert!(e.take_effects().is_empty());
        assert!(e.ui.wallpaper_invalid);
    }
    apply_ui(&mut e, &commands::registry(), vec![
        UiEvent::WallpaperColour("#AbC123".into()),
        UiEvent::Command("wallpaper.apply", Origin::Ui),
    ]);
    let (ticket, body) = set_after_preflight(&mut e, config);
    assert_eq!(body, json!({"schema":1,"default":{"colour":"#AbC123"}}));
    e.wallpaper_finished(ticket, reply(0, json!({"requested":2,"active":2})));
    let (ticket, _) = call(&mut e, "backdrop.state");
    e.wallpaper_finished(ticket, state(body));
    assert_eq!(e.wallpaper.status, "status.accepted");
    assert_eq!(prefs::label("wallpaper-accepted"), "Wallpaper accepted.");
}
