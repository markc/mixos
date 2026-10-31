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
    assert_eq!(
        changed_prefixes(&before, &after),
        BTreeSet::from(["time"])
    );
    let resolved = Resolved::default();
    let (a, b) = (contents(&before, &resolved), contents(&after, &resolved));
    let moved: Vec<_> = PREFIXES.iter().filter(|p| a[*p] != b[*p]).collect();
    assert_eq!(moved, [&"time"]);
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
