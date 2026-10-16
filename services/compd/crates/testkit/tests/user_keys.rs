//! The user-key layer on compd's real seat keyboard: xkb, the layer
//! (`world::comp::bindings::user_key`, the call the seat's key path makes),
//! the focused client's `wl_keyboard`, the keymap file loader, and the
//! workspace model a `comp.workspace.switch` binding drives. The layer's
//! pure rules are covered by policy's own `user_keys` tests.

use std::path::PathBuf;

use comp_model::request::{Request, WindowOp, classify};
use policy::workspaces::DefaultOutput;
use testkit::Harness;

/// evdev codes, and their xkb keycodes (evdev + 8) as the seat sees them.
const EV_RCTRL: u32 = 97;
const EV_LCTRL: u32 = 29;
const EV_RSHIFT: u32 = 54;
const EV_RALT: u32 = 100;
const EV_RIGHT: u32 = 106;
const EV_LEFT: u32 = 105;
const EV_F5: u32 = 63;
const EV_A: u32 = 30;
const F_KEYS: [u32; 12] = [59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88];
const ARROWS: [u32; 4] = [103, 105, 106, 108];

fn xkb(evdev: u32) -> u32 {
    evdev + 8
}

const WORKSPACE_KEYMAP: &str = r#"{"version":1,"physical":[
    {"stroke":{"code":105,"modifiers":{"right_ctrl":true}},"action":"comp.workspace.switch","args":{"index":"prev"}},
    {"stroke":{"code":106,"modifiers":{"right_ctrl":true}},"action":"comp.workspace.switch","args":{"index":"next"}}
]}"#;

/// A fresh directory for one test's keymap file.
fn keymap_file(test: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("compd-user-keys-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("keymap.json");
    std::fs::write(&path, text).unwrap();
    path
}

/// A harness whose user keymap is `text`, with one mapped toplevel holding
/// the keyboard and its key log empty.
fn harness(test: &str, text: &str) -> (Harness, PathBuf) {
    let mut h = Harness::new();
    h.wire.inner.comp.default_output = Some(DefaultOutput {
        key: "o_test".into(),
        name: "test".into(),
    });
    let path = keymap_file(test, text);
    h.wire.inner.comp.bindings.reload_user_keymap(&path);
    h.client.bind_keyboards();
    h.roundtrip();
    let (surface, _, _) = h.mapped_toplevel(64, 48);
    h.focus_keyboard(&surface);
    h.client.state.keys.clear();
    (h, path)
}

/// Run the `comp` verbs the layer queued as `policy_host::control::
/// apply_bindings` does: the Bus path's own parser, then the switch the
/// `comp.workspace.switch` verb runs. Returns how many ran.
fn run_comp_fires(h: &mut Harness) -> usize {
    let fires = h.wire.inner.comp.bindings.take_comp_fires();
    for fire in &fires {
        match classify(fire.verb, &fire.body, false) {
            Ok(Request::Window(WindowOp::SwitchWorkspace {
                output,
                index,
                wrap,
            })) => {
                let comp = &mut h.wire.inner.comp;
                let default = comp.default_output.clone();
                let _ = policy::workspaces::service_switch(
                    &mut comp.workspaces,
                    &comp.registry,
                    default.as_ref(),
                    output.as_deref(),
                    index,
                    wrap,
                );
            }
            other => panic!("unexpected request for {}: {other:?}", fire.verb),
        }
    }
    fires.len()
}

/// Press and release `keys` in order (the first is pressed first and
/// released last), returning whether the layer took each edge.
fn chord(h: &mut Harness, keys: &[u32]) -> Vec<bool> {
    let mut took = Vec::new();
    for &key in keys {
        took.push(h.key(xkb(key), true, false));
    }
    for &key in keys.iter().rev() {
        took.push(h.key(xkb(key), false, false));
    }
    took
}

#[test]
fn right_ctrl_right_switches_the_workspace_and_left_ctrl_right_reaches_the_client() {
    let (mut h, _) = harness("switch", WORKSPACE_KEYMAP);
    assert_eq!(h.comp().current_workspace(), 1);

    assert!(!h.key(xkb(EV_RCTRL), true, false), "the modifier goes on");
    assert!(h.key(xkb(EV_RIGHT), true, false), "Right Ctrl + → is taken");
    assert_eq!(run_comp_fires(&mut h), 1);
    assert_eq!(h.comp().current_workspace(), 2, "next");
    assert!(h.key(xkb(EV_RIGHT), false, false));
    assert!(!h.key(xkb(EV_RCTRL), false, false));
    assert_eq!(
        h.client.state.keys,
        vec![(EV_RCTRL, true), (EV_RCTRL, false)],
        "the client saw Right Ctrl and nothing else"
    );

    // Right Ctrl + ← goes back.
    chord(&mut h, &[EV_RCTRL, EV_LEFT]);
    assert_eq!(run_comp_fires(&mut h), 1);
    assert_eq!(h.comp().current_workspace(), 1, "prev");

    // Left Ctrl + → is the apps' word jump: unchanged, to the client.
    h.client.state.keys.clear();
    assert_eq!(chord(&mut h, &[EV_LCTRL, EV_RIGHT]), vec![false; 4]);
    assert_eq!(run_comp_fires(&mut h), 0);
    assert_eq!(h.comp().current_workspace(), 1);
    assert_eq!(
        h.client.state.keys,
        vec![
            (EV_LCTRL, true),
            (EV_RIGHT, true),
            (EV_RIGHT, false),
            (EV_LCTRL, false)
        ]
    );
}

/// §6.0's groups through the seat: every unbound bare F-key and every
/// right-modifier arrow group is swallowed, press and release, and the
/// client sees only the modifiers.
#[test]
fn unbound_user_keys_never_reach_the_client() {
    let (mut h, _) = harness("unbound", r#"{"version":1,"physical":[]}"#);
    for key in F_KEYS {
        assert_eq!(chord(&mut h, &[key]), vec![true, true], "F {key}");
    }
    assert!(h.client.state.keys.is_empty(), "{:?}", h.client.state.keys);
    for modifier in [EV_RCTRL, EV_RALT, EV_RSHIFT] {
        for arrow in ARROWS {
            assert_eq!(
                chord(&mut h, &[modifier, arrow]),
                vec![false, true, true, false]
            );
        }
    }
    assert!(
        h.client
            .state
            .keys
            .iter()
            .all(|(key, _)| [EV_RCTRL, EV_RALT, EV_RSHIFT].contains(key)),
        "{:?}",
        h.client.state.keys
    );
    assert_eq!(run_comp_fires(&mut h), 0);
    assert!(h.wire.inner.comp.bindings.take_bus_fires().is_empty());

    // Releasing the modifier first: the arrow's release is still swallowed.
    h.client.state.keys.clear();
    assert!(!h.key(xkb(EV_RCTRL), true, false));
    assert!(h.key(xkb(EV_LEFT), true, false));
    assert!(!h.key(xkb(EV_RCTRL), false, false));
    assert!(h.key(xkb(EV_LEFT), false, false), "latched at the press");
    assert_eq!(
        h.client.state.keys,
        vec![(EV_RCTRL, true), (EV_RCTRL, false)]
    );

    // Ordinary typing is untouched.
    h.client.state.keys.clear();
    assert_eq!(chord(&mut h, &[EV_A]), vec![false, false]);
    assert_eq!(h.client.state.keys, vec![(EV_A, true), (EV_A, false)]);
}

#[test]
fn a_passthrough_binding_reaches_the_client() {
    let (mut h, _) = harness(
        "passthrough",
        r#"{"version":1,"physical":[
            {"stroke":{"code":106,"modifiers":{"right_shift":true}},"action":"input.passthrough","passthrough":true}
        ]}"#,
    );
    assert_eq!(chord(&mut h, &[EV_RSHIFT, EV_RIGHT]), vec![false; 4]);
    assert_eq!(
        h.client.state.keys,
        vec![
            (EV_RSHIFT, true),
            (EV_RIGHT, true),
            (EV_RIGHT, false),
            (EV_RSHIFT, false)
        ]
    );
    assert!(h.wire.inner.comp.bindings.take_bus_fires().is_empty());
}

#[test]
fn a_session_lock_swallows_user_keys_and_fires_nothing() {
    let (mut h, _) = harness("lock", WORKSPACE_KEYMAP);
    assert!(!h.key(xkb(EV_RCTRL), true, true));
    assert!(h.key(xkb(EV_RIGHT), true, true));
    assert!(h.key(xkb(EV_RIGHT), false, true));
    assert!(!h.key(xkb(EV_RCTRL), false, true));
    assert!(h.key(xkb(EV_F5), true, true));
    assert!(h.key(xkb(EV_F5), false, true));
    assert_eq!(run_comp_fires(&mut h), 0);
    assert_eq!(h.comp().current_workspace(), 1);
    assert_eq!(h.wire.inner.comp.bindings.user_fired, 0);
}

#[test]
fn a_reload_while_a_key_is_held_applies_from_its_next_press() {
    let (mut h, path) = harness(
        "reload",
        r#"{"version":1,"physical":[{"stroke":{"code":63},"action":"user.f05"}]}"#,
    );
    assert!(h.key(xkb(EV_F5), true, false));
    let fired = h.wire.inner.comp.bindings.take_bus_fires();
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].service, "user", "the verb's first segment");
    assert_eq!(fired[0].verb, "user.f05");

    std::fs::write(
        &path,
        r#"{"version":1,"physical":[{"stroke":{"code":63},"action":"user.f05","passthrough":true}]}"#,
    )
    .unwrap();
    assert!(h.wire.inner.comp.bindings.reload_user_keymap(&path));
    assert!(
        h.key(xkb(EV_F5), false, false),
        "the release follows the press"
    );
    assert!(h.client.state.keys.is_empty());
    assert_eq!(
        chord(&mut h, &[EV_F5]),
        vec![false, false],
        "passthrough now"
    );
    assert_eq!(h.client.state.keys, vec![(EV_F5, true), (EV_F5, false)]);
    assert!(h.wire.inner.comp.bindings.take_bus_fires().is_empty());
}

#[test]
fn a_bad_file_keeps_the_keymap_and_a_missing_one_binds_nothing() {
    let (mut h, path) = harness("bad", WORKSPACE_KEYMAP);
    for bad in ["not json", r#"{"version":2,"physical":[]}"#, "[]"] {
        std::fs::write(&path, bad).unwrap();
        assert!(
            !h.wire.inner.comp.bindings.reload_user_keymap(&path),
            "{bad}"
        );
    }
    assert_eq!(h.wire.inner.comp.bindings.user.rows().len(), 2);
    chord(&mut h, &[EV_RCTRL, EV_RIGHT]);
    assert_eq!(
        run_comp_fires(&mut h),
        1,
        "the last good keymap is in force"
    );
    assert_eq!(h.comp().current_workspace(), 2);

    std::fs::remove_file(&path).unwrap();
    assert!(h.wire.inner.comp.bindings.reload_user_keymap(&path));
    assert!(h.wire.inner.comp.bindings.user.rows().is_empty());
    h.client.state.keys.clear();
    assert_eq!(
        chord(&mut h, &[EV_RCTRL, EV_RIGHT]),
        vec![false, true, true, false],
        "unbound: swallowed"
    );
    assert_eq!(run_comp_fires(&mut h), 0);
    assert_eq!(h.comp().current_workspace(), 2);
    assert_eq!(
        h.client.state.keys,
        vec![(EV_RCTRL, true), (EV_RCTRL, false)]
    );
}
