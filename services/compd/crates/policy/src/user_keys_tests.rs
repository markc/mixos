use super::*;
use inputd_core::inputd_schema::{ActionId, BindingScope, user_keys};
use serde_json::json;

const RCTRL: u32 = codes::KEY_RIGHTCTRL as u32 + XKB_OFFSET;
const LCTRL: u32 = codes::KEY_LEFTCTRL as u32 + XKB_OFFSET;
const RALT: u32 = codes::KEY_RIGHTALT as u32 + XKB_OFFSET;
const RSHIFT: u32 = codes::KEY_RIGHTSHIFT as u32 + XKB_OFFSET;
const LEFT: u32 = codes::KEY_LEFT as u32 + XKB_OFFSET;
const RIGHT: u32 = codes::KEY_RIGHT as u32 + XKB_OFFSET;
const P: u32 = codes::KEY_P as u32 + XKB_OFFSET;

fn xkb(code: u16) -> u32 {
    u32::from(code) + XKB_OFFSET
}

fn row(code: u16, modifiers: SideModifiers, action: &str, args: Value) -> PhysicalBinding {
    PhysicalBinding {
        stroke: PhysicalStroke { code, modifiers },
        action: ActionId::intern(action).unwrap(),
        args: Some(args),
        service: None,
        scope: BindingScope::Global,
        repeat: RepeatPolicy::Ignore,
        passthrough: false,
    }
}

/// Mark's first binding: Right Ctrl + ← / → steps the workspace.
fn workspace_rows() -> Vec<PhysicalBinding> {
    vec![
        row(
            codes::KEY_LEFT,
            SideModifiers::RIGHT_CTRL,
            "comp.workspace.switch",
            json!({"index": "prev"}),
        ),
        row(
            codes::KEY_RIGHT,
            SideModifiers::RIGHT_CTRL,
            "comp.workspace.switch",
            json!({"index": "next"}),
        ),
    ]
}

fn took(edge: &UserKeyEdge) -> bool {
    edge.took
}

#[test]
fn the_default_keymap_binds_nothing_and_swallows_every_user_key() {
    let mut keys = UserKeys::default();
    assert!(
        keys.rows().is_empty(),
        "no shipped default binds a user key"
    );
    for stroke in user_keys() {
        let held = held_modifiers(stroke.modifiers);
        for &modifier in &held {
            assert!(!took(&keys.edge(modifier, true, false)), "modifiers pass");
        }
        let press = keys.edge(xkb(stroke.code), true, false);
        assert!(press.took && press.fire.is_none() && press.repeat.is_none());
        assert!(keys.edge(xkb(stroke.code), false, false).took, "release");
        for &modifier in held.iter().rev() {
            assert!(!took(&keys.edge(modifier, false, false)));
        }
        assert!(keys.modifiers().is_empty());
    }
}

/// The xkb keycodes of the right-hand modifiers in `mods`.
fn held_modifiers(mods: SideModifiers) -> Vec<u32> {
    let mut out = Vec::new();
    if mods.right_ctrl {
        out.push(RCTRL);
    }
    if mods.right_alt {
        out.push(RALT);
    }
    if mods.right_shift {
        out.push(RSHIFT);
    }
    out
}

#[test]
fn each_right_modifier_arrow_group_is_swallowed_and_its_left_twin_is_not() {
    let mut keys = UserKeys::default();
    for modifier in [RCTRL, RALT, RSHIFT] {
        for arrow in codes::ARROWS {
            assert!(!keys.edge(modifier, true, false).took);
            assert!(keys.edge(xkb(arrow), true, false).took);
            assert!(keys.edge(xkb(arrow), false, false).took);
            assert!(!keys.edge(modifier, false, false).took);
        }
    }
    // Left Ctrl + arrow is the apps' word jump: it goes on.
    assert!(!keys.edge(LCTRL, true, false).took);
    assert!(!keys.edge(RIGHT, true, false).took);
    assert!(!keys.edge(RIGHT, false, false).took);
    assert!(!keys.edge(LCTRL, false, false).took);
    // Bare arrows and chorded F-keys too.
    assert!(!keys.edge(LEFT, true, false).took);
    assert!(!keys.edge(LEFT, false, false).took);
    assert!(!keys.edge(LCTRL, true, false).took);
    assert!(!keys.edge(xkb(61), true, false).took, "Ctrl+F3");
    assert!(!keys.edge(xkb(61), false, false).took);
    assert!(!keys.edge(LCTRL, false, false).took);
}

#[test]
fn a_bound_key_fires_on_press_only_and_routes_comp_verbs_inside() {
    let mut keys = UserKeys::new(workspace_rows());
    keys.edge(RCTRL, true, false);
    let press = keys.edge(RIGHT, true, false);
    assert!(press.took);
    let fire = press.fire.expect("the binding fires");
    assert_eq!(fire.service, "comp");
    assert!(fire.is_comp());
    assert_eq!(fire.verb, "comp.workspace.switch");
    assert_eq!(fire.body, json!({"index": "next"}));
    assert_eq!(press.repeat, None, "repeat: ignore");
    let release = keys.edge(RIGHT, false, false);
    assert!(release.took && release.fire.is_none());
}

#[test]
fn releasing_the_modifier_first_still_swallows_the_arrow_release() {
    let mut keys = UserKeys::new(workspace_rows());
    keys.edge(RCTRL, true, false);
    assert!(keys.edge(LEFT, true, false).fire.is_some());
    assert!(!keys.edge(RCTRL, false, false).took, "the modifier goes on");
    let release = keys.edge(LEFT, false, false);
    assert!(release.took, "latched at the press");
    assert!(release.fire.is_none());
    // And the next bare ← is the apps' again.
    assert!(!keys.edge(LEFT, true, false).took);
    assert!(!keys.edge(LEFT, false, false).took);
}

#[test]
fn a_rebind_or_reload_while_held_applies_from_the_next_press() {
    let mut keys = UserKeys::new(vec![row(63, SideModifiers::NONE, "user.f05", json!({}))]);
    assert_eq!(
        keys.edge(xkb(63), true, false).fire.unwrap().verb,
        "user.f05"
    );
    // Rebound to passthrough while held: the release still follows the press.
    let mut pass = row(63, SideModifiers::NONE, "user.f05", json!({}));
    pass.passthrough = true;
    assert!(keys.replace(vec![pass.clone()]));
    assert!(
        !keys.replace(vec![pass]),
        "an unchanged reload is no change"
    );
    assert!(keys.edge(xkb(63), false, false).took);
    assert!(!keys.edge(xkb(63), true, false).took, "passthrough now");
    // Removed while held (the file emptied): the release goes on as pressed.
    assert!(keys.replace(Vec::new()));
    assert!(!keys.edge(xkb(63), false, false).took);
    assert!(keys.edge(xkb(63), true, false).took, "unbound: swallowed");
    assert!(keys.edge(xkb(63), false, false).took);
}

#[test]
fn a_passthrough_row_goes_on_untouched() {
    let mut pass = row(
        codes::KEY_RIGHT,
        SideModifiers::RIGHT_SHIFT,
        "input.passthrough",
        json!({}),
    );
    pass.passthrough = true;
    let mut keys = UserKeys::new(vec![pass]);
    assert!(!keys.edge(RSHIFT, true, false).took);
    let press = keys.edge(RIGHT, true, false);
    assert!(!press.took && press.fire.is_none());
    assert!(!keys.edge(RIGHT, false, false).took);
    assert!(!keys.edge(RSHIFT, false, false).took);
}

#[test]
fn a_session_lock_swallows_user_keys_and_fires_nothing() {
    let mut pass = row(65, SideModifiers::NONE, "user.f07", json!({}));
    pass.passthrough = true;
    let mut rows = workspace_rows();
    rows.push(pass);
    let mut keys = UserKeys::new(rows);
    keys.edge(RCTRL, true, true);
    let press = keys.edge(RIGHT, true, true);
    assert!(press.took && press.fire.is_none() && press.repeat.is_none());
    assert!(keys.edge(RIGHT, false, true).took);
    keys.edge(RCTRL, false, true);
    // Even the user's passthrough key is held back from the lock surface.
    assert!(keys.edge(xkb(65), true, true).took);
    assert!(keys.edge(xkb(65), false, true).took);
    // Other keys reach the lock surface (Ctrl+Alt+Fn is not a user key).
    assert!(!keys.edge(P, true, true).took);
    assert!(!keys.edge(P, false, true).took);
    // A key pressed before the lock and released under it follows its press.
    keys.edge(RCTRL, true, false);
    assert!(keys.edge(LEFT, true, false).fire.is_some());
    assert!(keys.edge(LEFT, false, true).took);
}

#[test]
fn an_allow_row_repeats_until_release_and_an_ignore_row_never_does() {
    let mut step = row(
        codes::KEY_RIGHT,
        SideModifiers::RIGHT_ALT,
        "term.tab.next",
        json!({}),
    );
    step.repeat = RepeatPolicy::Allow;
    step.service = Some("term".into());
    let mut keys = UserKeys::new(vec![step]);
    keys.edge(RALT, true, false);
    let press = keys.edge(RIGHT, true, false);
    let token = press.repeat.expect("an allow row arms the repeat");
    assert_eq!(press.fire.as_ref().unwrap().service, "term");
    assert!(!press.fire.as_ref().unwrap().is_comp());
    for _ in 0..3 {
        assert_eq!(keys.repeat(token).unwrap().verb, "term.tab.next");
    }
    // Releasing the modifier does not stop the latched key.
    keys.edge(RALT, false, false);
    assert!(keys.repeat(token).is_some());
    keys.edge(RIGHT, false, false);
    assert_eq!(keys.repeat(token), None, "released");
    // Another key's press stops it too.
    keys.edge(RALT, true, false);
    let token = keys.edge(RIGHT, true, false).repeat.unwrap();
    keys.edge(P, true, false);
    assert_eq!(keys.repeat(token), None);
    // A stale token never fires a later press.
    keys.edge(RIGHT, false, false);
    let fresh = keys.edge(RIGHT, true, false).repeat.unwrap();
    assert_ne!(fresh, token);
    assert_eq!(keys.repeat(token), None);
    assert!(keys.repeat(fresh).is_some());
}

#[test]
fn reset_forgets_latches_and_modifiers() {
    let mut keys = UserKeys::new(workspace_rows());
    keys.edge(RCTRL, true, false);
    keys.edge(RIGHT, true, false);
    assert!(keys.is_swallowed(RIGHT));
    keys.reset();
    assert!(!keys.is_swallowed(RIGHT));
    assert!(keys.modifiers().is_empty(), "the pause took the keys up");
    // The release after the pause is an orphan: it goes on, as before.
    assert!(!keys.edge(RIGHT, false, false).took);
    // And a bare F-key is the user's again, not a chord.
    assert!(keys.edge(xkb(59), true, false).took);
}

#[test]
fn orphan_releases_and_odd_keycodes_go_on() {
    let mut keys = UserKeys::default();
    assert!(!keys.edge(xkb(59), false, false).took, "press never seen");
    assert_eq!(keys.edge(3, true, false), UserKeyEdge::default());
    assert_eq!(keys.edge(u32::MAX, true, false), UserKeyEdge::default());
}

#[test]
fn the_keymap_file_parses_the_inputd_format() {
    let text = r#"{"version":1,"physical":[
        {"stroke":{"code":106,"modifiers":{"right_ctrl":true}},"action":"comp.workspace.switch","args":{"index":"next"}},
        {"stroke":{"code":105,"modifiers":{"right_ctrl":true}},"action":"comp.workspace.switch","args":{"index":"prev"},"repeat":"allow"},
        {"stroke":{"code":25,"modifiers":{"left_ctrl":true,"left_alt":true}},"action":"scenes.editor.open"},
        {"stroke":{"code":63},"action":"bad action"},
        {"stroke":{"code":64},"action":"user.f06","service":"Bad"},
        {"stroke":{"code":65},"action":"user.f07","args":[1]},
        {"stroke":"nope","action":"x"}
    ]}"#;
    let loaded = parse_keymap(text).unwrap();
    assert_eq!(loaded.rows.len(), 2, "{loaded:?}");
    assert_eq!(loaded.dropped.len(), 5, "{:?}", loaded.dropped);
    assert!(loaded.dropped[0].contains("LeftCtrl+LeftAlt+code:25 is not a user key"));
    let keys = UserKeys::new(loaded.rows);
    let rows = keys.snapshot();
    assert_eq!(rows[0].chord, "RightCtrl+Right");
    assert_eq!(rows[0].action, "comp.workspace.switch");
    assert_eq!(rows[0].args, Some(json!({"index": "next"})));
    assert_eq!(rows[0].repeat, "ignore");
    assert_eq!(rows[1].chord, "RightCtrl+Left");
    assert_eq!(rows[1].repeat, "allow");
    assert!(!rows[1].passthrough);
}

#[test]
fn a_later_row_on_the_same_stroke_replaces_the_earlier() {
    let text = r#"{"version":1,"physical":[
        {"stroke":{"code":59},"action":"user.a"},
        {"stroke":{"code":59},"action":"user.b"}
    ]}"#;
    let loaded = parse_keymap(text).unwrap();
    assert_eq!(loaded.rows.len(), 1);
    assert_eq!(loaded.rows[0].action.as_str(), "user.b");
}

#[test]
fn a_bad_document_is_an_error() {
    for bad in [
        "",
        "not json",
        "[1, []]",
        r#"{"physical":[]}"#,
        r#"{"version":2,"physical":[]}"#,
        r#"{"version":1}"#,
        r#"{"version":1,"physical":{}}"#,
    ] {
        assert!(parse_keymap(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        parse_keymap(r#"{"version":1,"physical":[]}"#).unwrap(),
        LoadedKeymap::default()
    );
}

#[test]
fn chord_spellings() {
    let stroke = |code, modifiers| PhysicalStroke { code, modifiers };
    assert_eq!(chord(&stroke(59, SideModifiers::NONE)), "F1");
    assert_eq!(chord(&stroke(88, SideModifiers::NONE)), "F12");
    assert_eq!(
        chord(&stroke(
            codes::KEY_DOWN,
            SideModifiers {
                right_ctrl: true,
                right_shift: true,
                ..SideModifiers::NONE
            }
        )),
        "RightCtrl+RightShift+Down"
    );
}
