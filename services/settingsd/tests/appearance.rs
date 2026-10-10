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

/// Pinned from `resolve_with_embedded_and_accents` on the embedded design, printed
/// by a probe on the CBC worker (Studio, dark: the default desktop).
const STUDIO_DARK_ACCENT: [f64; 3] = [0.5450980392156862, 0.48627450980392156, 0.9647058823529412];
/// Pinned the same way, for `scheme = forest` in dark mode.
const FOREST_DARK_ACCENT: [f64; 3] = [
    0.07058823529411765,
    0.15294117647058825,
    0.09019607843137255,
];

/// The accent the desktop context draws, from the same compiled design the
/// authority resolves.
fn resolved_accent(desktop: &Desktop) -> [f64; 3] {
    let (_, accents) = resolve_with_embedded_and_accents(desktop, EMBEDDED_DEFAULT_SOURCE).unwrap();
    let colour = accents["desktop"];
    [colour.red, colour.green, colour.blue]
}

/// The accent the portal read serves for the current accepted state.
fn accent(authority: &mut Authority) -> [f64; 3] {
    projection(&service::dispatch(authority, "settings.appearance.get", &body("fixture")).unwrap())
        .accent
}

fn change_scheme(authority: &mut Authority, scheme: &str, operation: &str) {
    let apply = ApplyRequest {
        binding: binding(),
        expected_incarnation: authority.accepted.incarnation.clone(),
        expected_revision: authority.accepted.revision,
        operation_id: operation.into(),
        changes: BTreeMap::from([("appearance.scheme".into(), json!(scheme))]),
        reset: vec![],
        request_digest: None,
    };
    assert_eq!(authority.apply(apply).unwrap()["status"], "changed");
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

#[test]
fn the_served_accent_is_the_design_resolved_accent() {
    let dir = tempfile::tempdir().unwrap();
    let mut authority = authority(dir.path());
    assert_eq!(accent(&mut authority), STUDIO_DARK_ACCENT);
    assert_eq!(resolved_accent(&Desktop::default()), STUDIO_DARK_ACCENT);
}

#[test]
fn applying_a_scheme_change_serves_the_new_schemes_accent() {
    let dir = tempfile::tempdir().unwrap();
    let mut authority = authority(dir.path());
    assert_eq!(accent(&mut authority), STUDIO_DARK_ACCENT);
    change_scheme(&mut authority, "forest", "forest-1");
    let served = accent(&mut authority);
    assert_eq!(served, FOREST_DARK_ACCENT);
    assert_ne!(served, STUDIO_DARK_ACCENT);
    // The sidecar never enters the sealed snapshot: the served snapshot is still
    // the authority's own, and the accent surfaces only in the projection.
    let reply = service::dispatch(&mut authority, "settings.appearance.get", &body("fixture"))
        .expect("current appearance");
    let snapshot: Snapshot = serde_json::from_value(reply["snapshot"].clone()).unwrap();
    assert_eq!(snapshot, authority.snapshot);
}

#[test]
fn a_reopened_store_serves_the_same_accent() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut authority = authority(dir.path());
        change_scheme(&mut authority, "forest", "forest-1");
        assert_eq!(accent(&mut authority), FOREST_DARK_ACCENT);
    }
    // A fresh process: open the sealed state from disk and rebuild the authority,
    // which re-derives the accent sidecar from the accepted design.
    let (store, accepted) = Store::open(dir.path(), &binding()).unwrap();
    let mut reopened = Authority::new(store, accepted).unwrap();
    assert_eq!(accent(&mut reopened), FOREST_DARK_ACCENT);
}
