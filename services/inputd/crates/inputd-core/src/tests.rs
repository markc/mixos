// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

use super::*;
use inputd_schema::{codes::*, user_keys};

const KEY_F5: u16 = 63;
const KEY_A: u16 = 30;

fn mods(set: &[&str]) -> SideModifiers {
    let mut m = SideModifiers::NONE;
    for name in set {
        match *name {
            "lctrl" => m.left_ctrl = true,
            "rctrl" => m.right_ctrl = true,
            "lshift" => m.left_shift = true,
            "rshift" => m.right_shift = true,
            "lalt" => m.left_alt = true,
            "ralt" => m.right_alt = true,
            "lsuper" => m.left_super = true,
            "rsuper" => m.right_super = true,
            other => panic!("unknown modifier {other}"),
        }
    }
    m
}

fn row(code: u16, modifiers: SideModifiers, action: &'static str) -> PhysicalBinding {
    PhysicalBinding {
        stroke: PhysicalStroke { code, modifiers },
        action: ActionId::from_static(action),
        args: None,
        service: None,
        scope: BindingScope::default(),
        repeat: RepeatPolicy::Ignore,
        passthrough: false,
    }
}

fn passthrough(code: u16, modifiers: SideModifiers) -> PhysicalBinding {
    PhysicalBinding {
        passthrough: true,
        ..row(code, modifiers, "input.passthrough")
    }
}

/// The bindings a person migrating from the earlier broker would carry as
/// their own: workspace prev/next on Right Ctrl + ←/→ (repeating), and the
/// scene-editor defaults.
fn personal() -> Resolver {
    let mut r = Resolver::new(default_keymap());
    r.bind_physical(PhysicalBinding {
        repeat: RepeatPolicy::Allow,
        ..row(
            KEY_RIGHT,
            SideModifiers::RIGHT_CTRL,
            "desktop.workspace.next",
        )
    })
    .unwrap();
    r.bind_physical(PhysicalBinding {
        repeat: RepeatPolicy::Allow,
        ..row(
            KEY_LEFT,
            SideModifiers::RIGHT_CTRL,
            "desktop.workspace.prev",
        )
    })
    .unwrap();
    r
}

// ── Resolver ──

#[test]
fn a_bound_stroke_fires_and_swallows() {
    let r = personal();
    let out = r.resolve(KEY_RIGHT, SideModifiers::RIGHT_CTRL, Edge::Press);
    assert_eq!(
        out.verb.map(ActionId::as_str),
        Some("desktop.workspace.next")
    );
    assert!(out.swallow);
    let release = r.resolve(KEY_RIGHT, SideModifiers::RIGHT_CTRL, Edge::Release);
    assert_eq!(release.verb, None, "verbs fire on press, not release");
    assert!(release.swallow, "the release is swallowed too");
}

#[test]
fn matching_is_side_exact() {
    let r = personal();
    assert_eq!(
        r.resolve(KEY_RIGHT, mods(&["lctrl"]), Edge::Press),
        Resolution::PASS
    );
    assert_eq!(
        r.resolve(KEY_RIGHT, SideModifiers::NONE, Edge::Press),
        Resolution::PASS
    );
}

#[test]
fn repeat_honours_the_policy() {
    let mut r = personal();
    assert!(
        r.resolve(KEY_RIGHT, SideModifiers::RIGHT_CTRL, Edge::Repeat)
            .verb
            .is_some()
    );
    r.bind_physical(row(KEY_F5, SideModifiers::NONE, "term.snapshot"))
        .unwrap();
    let f5 = r.resolve(KEY_F5, SideModifiers::NONE, Edge::Repeat);
    assert_eq!(f5.verb, None, "ignore-policy rows do not repeat");
    assert!(f5.swallow);
}

#[test]
fn a_passthrough_row_re_emits() {
    let mut r = Resolver::new(default_keymap());
    r.bind_physical(passthrough(KEY_RIGHT, SideModifiers::RIGHT_SHIFT))
        .unwrap();
    assert_eq!(
        r.resolve(KEY_RIGHT, SideModifiers::RIGHT_SHIFT, Edge::Press),
        Resolution::PASS
    );
}

#[test]
fn transparent_mode_passes_everything_including_user_keys() {
    let mut r = personal();
    r.set_mode(InputMode::Transparent);
    assert_eq!(
        r.resolve(KEY_RIGHT, SideModifiers::RIGHT_CTRL, Edge::Press),
        Resolution::PASS
    );
    assert_eq!(
        r.resolve(KEY_F5, SideModifiers::NONE, Edge::Press),
        Resolution::PASS
    );
}

#[test]
fn rebind_and_unbind() {
    let mut r = Resolver::new(default_keymap());
    let g0 = r.generation();
    let g = r
        .bind_physical(row(KEY_F5, SideModifiers::NONE, "term.snapshot"))
        .unwrap();
    assert!(g > g0);
    let out = r.resolve(KEY_F5, SideModifiers::NONE, Edge::Press);
    assert_eq!(out.verb.map(ActionId::as_str), Some("term.snapshot"));
    let stroke = PhysicalStroke {
        code: KEY_F5,
        modifiers: SideModifiers::NONE,
    };
    assert!(r.unbind_physical(&stroke).is_some());
    assert_eq!(
        r.resolve(KEY_F5, SideModifiers::NONE, Edge::Press),
        Resolution::SWALLOW,
        "unbound again, the user key is swallowed, not passed"
    );
    assert!(r.unbind_physical(&stroke).is_none());
}

#[test]
fn args_ride_the_firing_edges_only() {
    let mut r = Resolver::new(default_keymap());
    r.bind_physical(PhysicalBinding {
        args: Some(serde_json::json!({"command": "kcalc"})),
        repeat: RepeatPolicy::Allow,
        ..row(KEY_F5, SideModifiers::NONE, "launch.run")
    })
    .unwrap();
    let press = r.resolve(KEY_F5, SideModifiers::NONE, Edge::Press);
    assert_eq!(press.args, Some(serde_json::json!({"command": "kcalc"})));
    let repeat = r.resolve(KEY_F5, SideModifiers::NONE, Edge::Repeat);
    assert_eq!(repeat.args, press.args);
    let release = r.resolve(KEY_F5, SideModifiers::NONE, Edge::Release);
    assert_eq!((release.verb, release.args), (None, None));
}

#[test]
fn admission_refuses_bad_args_and_targets() {
    let mut r = Resolver::new(default_keymap());
    let base = row(KEY_F5, SideModifiers::NONE, "launch.run");
    let big = PhysicalBinding {
        args: Some(serde_json::json!({"command": "x".repeat(MAX_ARGS_BYTES)})),
        ..base.clone()
    };
    assert_eq!(r.bind_physical(big), Err(BindError::ArgsTooLarge));
    let bare = PhysicalBinding {
        args: Some(serde_json::json!("kcalc")),
        ..base.clone()
    };
    assert_eq!(r.bind_physical(bare), Err(BindError::InvalidArgs));
    for name in [
        "",
        "a",
        "Desktop",
        "1desk",
        "-desk",
        "desk.vt1",
        "desk_vt1",
        "desk vt1",
        &"a".repeat(32),
    ] {
        let bad = PhysicalBinding {
            service: Some(name.to_string()),
            ..base.clone()
        };
        assert_eq!(
            r.bind_physical(bad),
            Err(BindError::InvalidService),
            "{name:?}"
        );
    }
    for name in ["desktop-vt1", "ab", "a-1", &"a".repeat(31)] {
        let good = PhysicalBinding {
            service: Some(name.to_string()),
            ..base.clone()
        };
        r.bind_physical(good)
            .unwrap_or_else(|e| panic!("{name:?}: {e:?}"));
        let out = r.resolve(KEY_F5, SideModifiers::NONE, Edge::Press);
        assert_eq!(out.service.as_deref(), Some(name));
    }
}

#[test]
fn capacity_is_bounded() {
    let mut r = Resolver::new(InputKeymap::default());
    for code in 0..MAX_PHYSICAL_ROWS as u16 {
        r.bind_physical(row(1000 + code, SideModifiers::NONE, "x.y"))
            .unwrap();
    }
    assert_eq!(
        r.bind_physical(row(5000, SideModifiers::NONE, "x.y")),
        Err(BindError::AtCapacity)
    );
    // Replacing an existing stroke is still allowed at capacity.
    r.bind_physical(row(1000, SideModifiers::NONE, "x.z"))
        .unwrap();
}

// ── The shipped keymap ──

#[test]
fn the_shipped_keymap_binds_no_user_key() {
    let shipped = default_keymap();
    for binding in &shipped.physical {
        assert!(
            !inputd_schema::is_user_key(&binding.stroke),
            "shipped row binds a user key: {binding:?}"
        );
    }
    // And through the resolver: every user key, every edge, is swallowed
    // with nothing fired, and no shipped row turns one into a passthrough.
    let r = Resolver::new(shipped);
    for stroke in user_keys() {
        for edge in [Edge::Press, Edge::Repeat, Edge::Release] {
            assert_eq!(
                r.resolve(stroke.code, stroke.modifiers, edge),
                Resolution::SWALLOW,
                "{stroke:?} {edge:?}"
            );
        }
    }
}

#[test]
fn the_shipped_keymap_is_only_the_scene_editor_chord() {
    let rows = default_keymap().physical;
    assert_eq!(rows.len(), 2);
    for (row, modifiers) in rows.iter().zip(SCENE_EDITOR_CHORDS) {
        assert_eq!(row.stroke.code, SCENE_EDITOR_KEY);
        assert_eq!(row.stroke.modifiers, modifiers);
        assert_eq!(row.action.as_str(), "scenes.editor.open");
        assert_eq!(row.service.as_deref(), Some(SCENES_SERVICE));
        assert_eq!(row.args, Some(serde_json::json!({"safe": true})));
        assert_eq!(row.repeat, RepeatPolicy::Ignore);
        assert!(!row.passthrough);
    }
    // Every shipped row passes the same admission as an input.bind.
    let mut fresh = Resolver::new(InputKeymap::default());
    for row in rows.clone() {
        fresh.bind_physical(row).unwrap();
    }
    assert_eq!(fresh.physical_rows(), rows.as_slice());
}

#[test]
fn the_scene_editor_chord_is_side_exact() {
    let r = Resolver::new(default_keymap());
    for held in SCENE_EDITOR_CHORDS {
        let press = r.resolve(SCENE_EDITOR_KEY, held, Edge::Press);
        assert_eq!(press.verb.map(ActionId::as_str), Some("scenes.editor.open"));
        assert!(press.swallow);
        let repeat = r.resolve(SCENE_EDITOR_KEY, held, Edge::Repeat);
        assert_eq!(
            (repeat.verb, repeat.swallow),
            (None, true),
            "no flap on repeat"
        );
    }
    for held in [
        mods(&["lctrl", "ralt"]),
        mods(&["rctrl", "ralt"]),
        mods(&[]),
        mods(&["lctrl"]),
        mods(&["lalt"]),
        mods(&["lctrl", "lalt", "lshift"]),
        mods(&["lctrl", "lalt", "lsuper"]),
    ] {
        assert_eq!(
            r.resolve(SCENE_EDITOR_KEY, held, Edge::Press),
            Resolution::PASS,
            "{held:?}"
        );
    }
}

// ── The interceptor: §6.0 tests ──

/// Drives an [`Interceptor`] and records what reaches the re-emitter.
struct Run {
    resolver: Resolver,
    keys: Interceptor,
    emitted: Vec<(u16, i32)>,
    fired: Vec<&'static str>,
}

impl Run {
    fn new(resolver: Resolver) -> Self {
        let mut keys = Interceptor::new();
        keys.begin(SideModifiers::NONE);
        Self {
            resolver,
            keys,
            emitted: Vec::new(),
            fired: Vec::new(),
        }
    }

    fn feed(&mut self, code: u16, value: i32) -> &mut Self {
        let step = self.keys.key(&self.resolver, code, value);
        if step.emit {
            self.emitted.push((code, value));
        }
        if let Some(fire) = step.fire {
            self.fired.push(fire.verb.as_str());
        }
        self
    }

    fn tap_chord(&mut self, modifiers: &[u16], key: u16) -> &mut Self {
        for &m in modifiers {
            self.feed(m, 1);
        }
        self.feed(key, 1).feed(key, 2).feed(key, 2).feed(key, 0);
        for &m in modifiers.iter().rev() {
            self.feed(m, 0);
        }
        self
    }

    fn emitted_codes(&self) -> Vec<u16> {
        self.emitted.iter().map(|&(code, _)| code).collect()
    }

    /// Every press that reached the re-emitter has its release there too.
    fn assert_balanced(&self) {
        let mut down = std::collections::BTreeSet::new();
        for &(code, value) in &self.emitted {
            match value {
                1 => assert!(down.insert(code), "double press of {code}"),
                0 => assert!(
                    down.remove(&code),
                    "orphan release of {code} reached the re-emitter"
                ),
                _ => assert!(down.contains(&code), "repeat of {code} without its press"),
            }
        }
        assert!(down.is_empty(), "left held on the re-emitter: {down:?}");
    }
}

const RIGHT_MODS: [(u16, &str); 3] = [
    (KEY_RIGHTCTRL, "right ctrl"),
    (KEY_RIGHTALT, "right alt"),
    (KEY_RIGHTSHIFT, "right shift"),
];

#[test]
fn unbound_f_keys_are_swallowed_on_every_edge() {
    let mut run = Run::new(Resolver::new(default_keymap()));
    for code in F_KEYS {
        run.feed(code, 1).feed(code, 2).feed(code, 2).feed(code, 0);
    }
    assert!(run.emitted.is_empty(), "leaked: {:?}", run.emitted);
    assert!(run.fired.is_empty(), "fired: {:?}", run.fired);
}

#[test]
fn unbound_right_modifier_arrows_are_swallowed_on_every_edge() {
    for (modifier, name) in RIGHT_MODS {
        let mut run = Run::new(Resolver::new(default_keymap()));
        for arrow in ARROWS {
            run.tap_chord(&[modifier], arrow);
        }
        // Only the modifier itself reaches the re-emitter.
        assert!(
            run.emitted_codes().iter().all(|&c| c == modifier),
            "{name}: leaked {:?}",
            run.emitted
        );
        assert!(run.fired.is_empty(), "{name}: fired {:?}", run.fired);
        run.assert_balanced();
    }
}

#[test]
fn a_bound_user_key_fires_and_never_reaches_the_re_emitter() {
    let mut run = Run::new(personal());
    run.tap_chord(&[KEY_RIGHTCTRL], KEY_RIGHT);
    assert_eq!(
        run.fired, ["desktop.workspace.next"; 3],
        "press plus two allowed repeats"
    );
    assert!(!run.emitted_codes().contains(&KEY_RIGHT));
    run.assert_balanced();
}

#[test]
fn releasing_the_modifier_first_keeps_the_arrow_swallowed() {
    for (modifier, name) in RIGHT_MODS {
        let mut run = Run::new(Resolver::new(default_keymap()));
        run.feed(modifier, 1)
            .feed(KEY_LEFT, 1)
            .feed(modifier, 0)
            // Repeats and the release now resolve under no modifier, a bare
            // ← that would pass; the latch keeps them swallowed.
            .feed(KEY_LEFT, 2)
            .feed(KEY_LEFT, 0);
        assert_eq!(run.emitted, [(modifier, 1), (modifier, 0)], "{name}");
        run.assert_balanced();
    }
    // The same with a bound key: the verb fires once and its release stays
    // swallowed.
    let mut run = Run::new(personal());
    run.feed(KEY_RIGHTCTRL, 1)
        .feed(KEY_LEFT, 1)
        .feed(KEY_RIGHTCTRL, 0)
        .feed(KEY_LEFT, 0);
    assert_eq!(run.fired, ["desktop.workspace.prev"]);
    assert!(!run.emitted_codes().contains(&KEY_LEFT));
}

#[test]
fn a_modifier_pressed_mid_hold_never_changes_the_latch() {
    // Bare ← is the app's: emitted at press. Right Ctrl arriving later must
    // not turn its repeat or release into a swallowed user key.
    let mut run = Run::new(personal());
    run.feed(KEY_LEFT, 1)
        .feed(KEY_RIGHTCTRL, 1)
        .feed(KEY_LEFT, 2)
        .feed(KEY_LEFT, 0)
        .feed(KEY_RIGHTCTRL, 0);
    assert!(run.fired.is_empty());
    assert_eq!(
        run.emitted,
        [
            (KEY_LEFT, 1),
            (KEY_RIGHTCTRL, 1),
            (KEY_LEFT, 2),
            (KEY_LEFT, 0),
            (KEY_RIGHTCTRL, 0)
        ]
    );
    run.assert_balanced();
}

#[test]
fn a_rebind_while_held_applies_from_the_next_press() {
    // Swallowed at press, made passthrough while held: the release stays
    // swallowed; the next press is emitted.
    let mut run = Run::new(Resolver::new(default_keymap()));
    run.feed(KEY_F5, 1);
    run.resolver
        .bind_physical(passthrough(KEY_F5, SideModifiers::NONE))
        .unwrap();
    run.feed(KEY_F5, 2).feed(KEY_F5, 0);
    assert!(run.emitted.is_empty(), "leaked {:?}", run.emitted);
    run.feed(KEY_F5, 1).feed(KEY_F5, 0);
    assert_eq!(run.emitted, [(KEY_F5, 1), (KEY_F5, 0)]);

    // Emitted at press (passthrough), unbound while held: the release still
    // goes to the re-emitter, so nothing is left held there.
    run.feed(KEY_F5, 1);
    run.resolver
        .unbind_physical(&PhysicalStroke {
            code: KEY_F5,
            modifiers: SideModifiers::NONE,
        })
        .unwrap();
    run.feed(KEY_F5, 2).feed(KEY_F5, 0);
    assert_eq!(run.emitted[2..], [(KEY_F5, 1), (KEY_F5, 2), (KEY_F5, 0)]);
    run.assert_balanced();

    // Bound to one verb at press, rebound while held: repeats keep firing
    // the verb latched at the press.
    let mut run = Run::new(personal());
    run.feed(KEY_RIGHTCTRL, 1).feed(KEY_RIGHT, 1);
    run.resolver
        .bind_physical(row(KEY_RIGHT, SideModifiers::RIGHT_CTRL, "other.verb"))
        .unwrap();
    run.feed(KEY_RIGHT, 2).feed(KEY_RIGHT, 0);
    assert_eq!(
        run.fired,
        ["desktop.workspace.next", "desktop.workspace.next"]
    );
    run.feed(KEY_RIGHT, 1)
        .feed(KEY_RIGHT, 0)
        .feed(KEY_RIGHTCTRL, 0);
    assert_eq!(run.fired.last(), Some(&"other.verb"));
    run.assert_balanced();
}

#[test]
fn a_hand_off_while_held_releases_only_what_was_emitted() {
    let mut run = Run::new(Resolver::new(default_keymap()));
    run.resolver
        .bind_physical(passthrough(KEY_UP, SideModifiers::RIGHT_SHIFT))
        .unwrap();
    // Held at switch-away: an ordinary letter, a bare F5 (unbound, so
    // swallowed), and Right Shift + ↑ passed through by the person's choice.
    run.feed(KEY_A, 1)
        .feed(KEY_F5, 1)
        .feed(KEY_RIGHTSHIFT, 1)
        .feed(KEY_UP, 1);
    assert!(run.keys.is_swallowed(KEY_F5));
    let to_release = run.keys.end();
    assert_eq!(to_release, [KEY_A, KEY_RIGHTSHIFT, KEY_UP]);
    for code in to_release {
        run.emitted.push((code, 0));
    }
    run.assert_balanced();
    assert!(run.keys.mods().is_empty());

    // Back under a new grab: the physical releases of those keys are
    // orphans and reach nothing.
    run.keys.begin(SideModifiers::NONE);
    let before = run.emitted.len();
    run.feed(KEY_UP, 2)
        .feed(KEY_UP, 0)
        .feed(KEY_RIGHTSHIFT, 0)
        .feed(KEY_A, 0)
        .feed(KEY_F5, 0);
    assert_eq!(
        run.emitted.len(),
        before,
        "orphans leaked: {:?}",
        &run.emitted[before..]
    );
    assert!(run.fired.is_empty());
}

#[test]
fn a_modifier_held_across_the_grab_still_forms_the_chord() {
    // Right Ctrl went down before the grab; ← pressed under it is the user
    // key, and the Right Ctrl release is an orphan.
    let mut run = Run::new(personal());
    run.keys.begin(SideModifiers::RIGHT_CTRL);
    run.feed(KEY_RIGHT, 1)
        .feed(KEY_RIGHT, 0)
        .feed(KEY_RIGHTCTRL, 0);
    assert_eq!(run.fired, ["desktop.workspace.next"]);
    assert!(run.emitted.is_empty(), "{:?}", run.emitted);
    assert!(run.keys.mods().is_empty());
}

#[test]
fn ordinary_typing_passes_untouched() {
    let mut run = Run::new(personal());
    run.tap_chord(&[KEY_LEFTSHIFT], KEY_A)
        .tap_chord(&[KEY_LEFTCTRL], KEY_RIGHT);
    assert!(run.fired.is_empty());
    assert_eq!(run.emitted.len(), 12, "{:?}", run.emitted);
    run.assert_balanced();
}

#[test]
fn a_second_press_without_release_keeps_the_first_decision() {
    let mut run = Run::new(Resolver::new(default_keymap()));
    run.feed(KEY_A, 1).feed(KEY_A, 1).feed(KEY_A, 0);
    assert_eq!(run.emitted, [(KEY_A, 1), (KEY_A, 1), (KEY_A, 0)]);
    assert!(!run.keys.is_emitted(KEY_A), "one release clears the latch");
    let mut run = Run::new(personal());
    run.feed(KEY_RIGHTCTRL, 1)
        .feed(KEY_RIGHT, 1)
        .feed(KEY_RIGHT, 1)
        .feed(KEY_RIGHT, 0);
    assert_eq!(
        run.fired.len(),
        2,
        "the re-press fired as an allowed repeat"
    );
    assert!(!run.emitted_codes().contains(&KEY_RIGHT));
}

#[test]
fn a_resync_after_dropped_events_releases_what_is_up_and_rebuilds_modifiers() {
    let mut run = Run::new(Resolver::new(default_keymap()));
    // Held: A (emitted), F5 (swallowed), Right Ctrl (emitted). The device
    // then drops events, losing the releases of A, F5 and Right Ctrl, and
    // the press of Left Shift.
    run.feed(KEY_A, 1).feed(KEY_F5, 1).feed(KEY_RIGHTCTRL, 1);
    let released = run.keys.resync(&[KEY_LEFTSHIFT], mods(&["lshift"]));
    assert_eq!(released, [KEY_A, KEY_RIGHTCTRL], "every emitted key");
    assert!(!run.keys.is_emitted(KEY_A) && !run.keys.is_swallowed(KEY_F5));
    assert!(run.keys.is_swallowed(KEY_LEFTSHIFT), "held now: quarantined");
    assert_eq!(run.keys.mods(), mods(&["lshift"]));
    // The stale Right Ctrl is gone: a bare-arrow chord is not formed, and
    // Left Shift (whose press was lost) still counts for matching.
    for code in released {
        run.emitted.push((code, 0));
    }
    run.feed(KEY_LEFTSHIFT, 0);
    run.feed(KEY_F5, 1).feed(KEY_F5, 0);
    assert!(
        !run.emitted_codes().contains(&KEY_F5),
        "bare F5 after resync is the user's: {:?}",
        run.emitted
    );
    run.assert_balanced();
}

#[test]
fn a_resync_releases_and_quarantines_keys_still_down() {
    // RCtrl + F5 is emitted (a chorded F-key is the apps'). Inside the lost
    // events F5 and Right Ctrl go up and a NEW bare F5 goes down. The state
    // shows F5 down, but that is not the press latched as emitted.
    let mut run = Run::new(Resolver::new(default_keymap()));
    run.feed(KEY_RIGHTCTRL, 1).feed(KEY_F5, 1);
    assert_eq!(run.emitted, [(KEY_RIGHTCTRL, 1), (KEY_F5, 1)]);
    let released = run.keys.resync(&[KEY_F5], SideModifiers::NONE);
    assert_eq!(released, [KEY_F5, KEY_RIGHTCTRL], "released though F5 is down");
    for code in released {
        run.emitted.push((code, 0));
    }
    // The new bare F5's repeats and release reach nothing, and fire nothing.
    run.feed(KEY_F5, 2).feed(KEY_F5, 2).feed(KEY_F5, 0);
    run.assert_balanced();
    assert_eq!(run.emitted.len(), 4, "{:?}", run.emitted);
    assert!(run.fired.is_empty());
    // Quarantine ends at the release: the next press is decided afresh.
    run.feed(KEY_A, 1).feed(KEY_A, 0);
    assert_eq!(run.emitted[4..], [(KEY_A, 1), (KEY_A, 0)]);
}

#[test]
fn an_orphan_is_flagged_and_a_quarantined_key_is_not() {
    let mut keys = Interceptor::new();
    let resolver = Resolver::new(default_keymap());
    keys.begin(SideModifiers::NONE);
    for (code, value) in [(KEY_A, 2), (KEY_A, 0), (KEY_RIGHTCTRL, 0)] {
        let step = keys.key(&resolver, code, value);
        assert!(step.orphan && !step.emit, "{code} {value}");
    }
    keys.resync(&[KEY_A], SideModifiers::NONE);
    let step = keys.key(&resolver, KEY_A, 0);
    assert!(!step.orphan && !step.emit);
}
