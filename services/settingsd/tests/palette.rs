// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `settings.palette` (key space v2 §5.2): the desktop look's palette per
//! mode, from `design::palette`, tagged with the record's identity.
use serde_json::{Value, json};
use settings::*;
use settingsd::{authority::Authority, service::dispatch, store::Store};
use std::collections::BTreeMap;

fn binding() -> Binding {
    Binding {
        instance: "fixture".into(),
        profile: "default".into(),
    }
}

fn authority(dir: &std::path::Path, values: registry::Values) -> Authority {
    let (store, data) = Store::create(dir, binding(), values).unwrap();
    Authority::new(store, data).unwrap()
}

fn palette(authority: &mut Authority, body: Value) -> Result<Value, Value> {
    dispatch(authority, "settings.palette", &body.to_string())
}

#[test]
fn both_modes_by_default_one_when_asked_tagged_with_the_record() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = authority(dir.path(), registry::Values::new());
    let both = palette(&mut state, json!({"binding": binding()})).unwrap();
    assert_eq!(both["status"], "current");
    assert_eq!(both["incarnation"], json!(state.accepted.incarnation));
    assert_eq!(both["revision"], json!(state.accepted.revision));
    for mode in ["dark", "light"] {
        let one = &both["palette"][mode];
        assert_eq!(one["mode"], mode);
        assert_eq!(one["scheme"], "mixos", "a new profile starts on mixos (F5)");
        assert_eq!(one["parity"], "exact");
        assert_eq!(one["roles"].as_object().unwrap().len(), 31);
        assert_eq!(one["swatches"].as_object().unwrap().len(), 9);
        assert!(one["cosmic"]["theme"].is_object());
        assert!(one["accent"].is_string());
    }
    let light = palette(&mut state, json!({"binding": binding(), "mode": "light"})).unwrap();
    let modes: Vec<&String> = light["palette"].as_object().unwrap().keys().collect();
    assert_eq!(modes, ["light"]);
    assert_eq!(light["palette"]["light"], both["palette"]["light"]);
}

#[test]
fn it_follows_the_authored_look() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = authority(
        dir.path(),
        registry::Values::from([
            ("appearance.scheme".into(), json!("studio")),
            ("appearance.contrast".into(), json!("high")),
        ]),
    );
    let answer = palette(&mut state, json!({"binding": binding(), "mode": "dark"})).unwrap();
    let dark = &answer["palette"]["dark"];
    assert_eq!(dark["scheme"], "studio");
    assert_eq!(dark["contrast"], "high");
    assert_eq!(dark["parity"], "closest");

    // A change moves the answer's revision with it.
    let before = answer["revision"].clone();
    state
        .apply(ApplyRequest {
            binding: binding(),
            expected_incarnation: state.accepted.incarnation.clone(),
            expected_revision: state.accepted.revision,
            operation_id: "accent".into(),
            changes: BTreeMap::from([("appearance.contrast".into(), json!("normal"))]),
            reset: vec![],
            request_digest: None,
        })
        .unwrap();
    let after = palette(&mut state, json!({"binding": binding(), "mode": "dark"})).unwrap();
    assert_ne!(after["revision"], before);
    assert_eq!(after["palette"]["dark"]["contrast"], "normal");
}

#[test]
fn it_refuses_another_binding_and_an_unknown_mode() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = authority(dir.path(), registry::Values::new());
    let other = json!({"binding": {"instance": "other", "profile": "default"}});
    assert_eq!(
        palette(&mut state, other).unwrap_err()["status"],
        "wrong_target"
    );
    let bad = json!({"binding": binding(), "mode": "auto"});
    assert_eq!(
        palette(&mut state, bad).unwrap_err()["status"],
        "validation_failed"
    );
    // A read: listed in the manifest beside the other reads.
    assert!(
        settingsd::service::COMMANDS.contains(&"settings.palette"),
        "served"
    );
}
