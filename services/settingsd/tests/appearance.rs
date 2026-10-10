// SPDX-License-Identifier: MIT OR Apache-2.0
use serde_json::{Value, json};
use settings::appearance::{APPEARANCE_SCHEMA, AppearanceProjection};
use settings::*;
use settingsd::{authority::Authority, service, store::Store};
use std::collections::BTreeMap;

fn binding() -> Binding {
    Binding {
        instance: "fixture".into(),
        profile: "default".into(),
    }
}
fn authority(dir: &std::path::Path) -> Authority {
    let (store, data) = Store::create(dir, binding(), Desktop::default()).unwrap();
    Authority::new(store, data).unwrap()
}
fn body(instance: &str) -> String {
    json!({"binding": {"instance": instance, "profile": "default"}}).to_string()
}
fn projection(value: &Value) -> AppearanceProjection {
    serde_json::from_value(value["appearance"].clone()).unwrap()
}

#[test]
fn appearance_projects_the_desktop_context_through_the_verb() {
    let dir = tempfile::tempdir().unwrap();
    let mut authority = authority(dir.path());
    let reply = service::dispatch(&mut authority, "settings.appearance.get", &body("fixture"))
        .expect("current appearance");
    assert_eq!(reply["status"], "current");
    let appearance = projection(&reply);
    assert_eq!(appearance.schema, APPEARANCE_SCHEMA);
    assert_eq!(appearance.mode, "dark");
    assert_eq!(appearance.contrast, "normal");
    assert_eq!(appearance.revision, authority.accepted.revision);
    assert_eq!(appearance.incarnation, authority.accepted.incarnation);
    let snapshot: Snapshot = serde_json::from_value(reply["snapshot"].clone()).unwrap();
    assert_eq!(snapshot, authority.snapshot);
    assert_eq!(
        AppearanceProjection::from_snapshot(&snapshot, appearance.accent).unwrap(),
        appearance
    );
    appearance.validate().unwrap();
}

#[test]
fn appearance_refuses_a_foreign_binding_and_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut authority = authority(dir.path());
    let wrong = service::dispatch(
        &mut authority,
        "settings.appearance.get",
        &body("elsewhere"),
    )
    .unwrap_err();
    assert_eq!(wrong["status"], "wrong_target");
    let extra =
        json!({"binding": {"instance": "fixture", "profile": "default"}, "x": 1}).to_string();
    let refused = service::dispatch(&mut authority, "settings.appearance.get", &extra).unwrap_err();
    assert_eq!(refused["status"], "validation_failed");
}

#[test]
fn appearance_follows_an_accepted_mode_change_with_a_new_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut authority = authority(dir.path());
    let before = projection(
        &service::dispatch(&mut authority, "settings.appearance.get", &body("fixture")).unwrap(),
    );
    let apply = ApplyRequest {
        binding: binding(),
        expected_incarnation: authority.accepted.incarnation.clone(),
        expected_revision: authority.accepted.revision,
        operation_id: "light-1".into(),
        changes: BTreeMap::from([("appearance.mode".into(), json!("light"))]),
        reset: vec![],
        request_digest: None,
    };
    let applied = authority.apply(apply).unwrap();
    assert_eq!(applied["status"], "changed");
    let after = projection(
        &service::dispatch(&mut authority, "settings.appearance.get", &body("fixture")).unwrap(),
    );
    assert_eq!(after.mode, "light");
    assert_ne!(after.identity(), before.identity());
    after.validate().unwrap();
}

#[test]
fn appearance_verb_is_registered_as_read_only() {
    let manifest = service::manifest();
    let verb = manifest
        .iter()
        .find(|descriptor| descriptor.name == "settings.appearance.get")
        .expect("verb in manifest");
    assert!(verb.read_only);
}
