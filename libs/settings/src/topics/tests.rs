// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use super::*;
use serde_json::json;

fn authored(changes: Value) -> Values {
    let changes: BTreeMap<String, Value> = serde_json::from_value(changes).unwrap();
    registry::apply(&Values::new(), &changes, &[]).unwrap()
}

#[test]
fn every_stored_key_has_exactly_one_topic_and_every_topic_has_a_key() {
    for def in registry::keys() {
        let prefix = prefix_of(def.path);
        if def.class == Class::Live {
            // A live entry may sit under a prefix that also has stored keys
            // (`sound`, `power`), but it is never in a body.
            continue;
        }
        assert!(prefix.is_some(), "{} is in no topic", def.path);
    }
    for prefix in PREFIXES {
        assert!(
            registry::keys()
                .iter()
                .any(|d| d.class != Class::Live && prefix_of(d.path) == Some(prefix)),
            "{prefix} has no stored key"
        );
    }
    assert_eq!(
        PREFIXES.iter().collect::<BTreeSet<_>>().len(),
        22,
        "twenty-two topics"
    );
}

#[test]
fn prefixes_match_whole_segments_and_the_longest_wins() {
    assert_eq!(prefix_of("apps_default.web"), Some("apps_default"));
    assert_eq!(prefix_of("apps.term.mode"), Some("apps"));
    assert_eq!(prefix_of("shell.panels.top.mode"), Some("shell.panels"));
    assert_eq!(prefix_of("time.applet.show_seconds"), Some("time"));
    assert_eq!(prefix_of("timer.x"), None);
    assert_eq!(prefix_of("network.wifi.enabled"), None);
    assert_eq!(prefix_of("system.hostname"), None);
    assert_eq!(
        keys_topic("default", "shell.panels"),
        "settingsd.desktop.keys.default.shell.panels"
    );
}

#[test]
fn bodies_fill_defaults_and_leave_live_keys_out() {
    let empty = Values::new();
    let time = prefix_values(&empty, "time");
    assert_eq!(time["time.applet.show_seconds"], json!(false));
    // Owner-supplied and never authored: the owner knows it.
    assert_eq!(time["time.zone"], Value::Null);
    let sound = prefix_values(&empty, "sound");
    assert_eq!(sound["sound.output.amplify"], json!(true));
    assert!(!sound.contains_key("sound.output.volume"), "{sound:?}");
    // Closed placeholders list every member.
    let panels = prefix_values(&empty, "shell.panels");
    assert!(panels.contains_key("shell.panels.top.autohide"));
    assert!(panels.contains_key("shell.panels.bottom.autohide"));
    // App domains take their module's default.
    assert_eq!(
        prefix_values(&empty, "terminal")["terminal.padding"],
        json!(8)
    );
}

#[test]
fn open_families_list_each_authored_member_with_all_its_keys() {
    assert!(prefix_values(&Values::new(), "displays").is_empty());
    let values = authored(json!({"displays.HDMI-A-1.scale": 150}));
    let displays = prefix_values(&values, "displays");
    assert_eq!(displays["displays.HDMI-A-1.scale"], json!(150));
    assert_eq!(displays["displays.HDMI-A-1.rotation"], json!("normal"));
    assert_eq!(displays["displays.HDMI-A-1.enabled"], json!(true));
    assert_eq!(displays.len(), 7, "{displays:?}");
}

#[test]
fn a_change_moves_only_its_prefix() {
    let before = authored(json!({"appearance.mode": "light"}));
    let mut changes = BTreeMap::new();
    changes.insert("time.applet.show_seconds".to_owned(), json!(true));
    let after = registry::apply(&before, &changes, &[]).unwrap();
    assert_eq!(changed_prefixes(&before, &after), BTreeSet::from(["time"]));
    let resolved = Resolved::default();
    let (a, b) = (contents(&before, &resolved), contents(&after, &resolved));
    let moved: Vec<_> = PREFIXES.iter().filter(|p| a[*p] != b[*p]).collect();
    assert_eq!(moved, [&"time"]);
}

#[test]
fn the_values_view_reads_a_prefix_or_keys_and_refuses_live_paths() {
    let empty = Values::new();
    let time = view_values(&empty, Some("time.applet"), None).unwrap();
    assert_eq!(time.len(), 3, "{time:?}");
    assert_eq!(time["time.applet.show_seconds"], json!(false));
    let keys = ["appearance.mode".to_owned(), "window.gaps".to_owned()];
    let picked = view_values(&empty, None, Some(&keys)).unwrap();
    assert_eq!(picked.len(), 2);
    let live = ["sound.output.volume".to_owned()];
    let refused = view_values(&empty, None, Some(&live)).unwrap_err();
    assert_eq!(refused[0].code, "live_key");
    let refused = view_values(&empty, Some("system"), None).unwrap_err();
    assert_eq!(refused[0].code, "live_key");
    assert!(view_values(&empty, Some("nope"), None).is_err());
    assert!(
        view_values(&empty, Some("displays"), None)
            .unwrap()
            .is_empty()
    );
    let all = view_values(&empty, None, None).unwrap();
    assert!(all.keys().all(|k| prefix_of(k).is_some()));
}

#[test]
fn a_follower_reads_the_look_back_with_the_same_views() {
    use crate::view::{Appearance, Framing, State, View};
    let values = authored(json!({
        "appearance.mode": "auto",
        "state.appearance.daylight": true,
        "accessibility.text_scale": 1.5,
        "time.applet.show_seconds": true
    }));
    let resolved = Resolved {
        mode: resolved_mode(&values),
        accent: None,
    };
    let look = look_content(&values, &resolved, "digest");
    assert!(look.values.keys().all(|k| in_look(k)), "{:?}", look.values);
    assert!(!look.values.contains_key("time.applet.show_seconds"));
    assert!(!look.values.contains_key("window.gaps"));
    assert_eq!(look.values["accessibility.text_scale"], json!(1.5));
    assert_eq!(look.resolved["appearance.mode"], json!("light"));
    let rebuilt: Values = look.values.into_iter().collect();
    assert_eq!(Appearance::of(&rebuilt), Appearance::of(&values));
    assert_eq!(Framing::of(&rebuilt), Framing::of(&values));
    assert_eq!(State::of(&rebuilt), State::of(&values));
}

#[test]
fn the_mode_resolves_beside_the_authored_choice() {
    assert_eq!(resolved_mode(&Values::new()).as_deref(), Some("dark"));
    let values = authored(json!({"appearance.mode": "auto", "state.appearance.daylight": true}));
    assert_eq!(resolved_mode(&values).as_deref(), Some("light"));
    let resolved = Resolved {
        mode: resolved_mode(&values),
        accent: Some("#3584e4".into()),
    };
    let appearance = &contents(&values, &resolved)["appearance"];
    assert_eq!(appearance.values["appearance.mode"], json!("auto"));
    let both = appearance.resolved.as_ref().unwrap();
    assert_eq!(both["appearance.mode"], json!("light"));
    assert_eq!(both["appearance.accent"], json!("#3584e4"));
    assert!(contents(&values, &resolved)["time"].resolved.is_none());
}
