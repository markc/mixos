// SPDX-License-Identifier: MIT OR Apache-2.0
//! The sealed-profile upgrade rule (`Accepted::upgrade`, `store::FORMAT`),
//! against a format-1 profile written by the settingsd before the style and
//! framing keys (fixtures: a primary and its backup after four accepted
//! changes, the shape a long-running desktop profile has).
use serde_json::json;
use settings::*;
use settingsd::{
    authority::Authority,
    store::{FORMAT, Store},
};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

const PRIMARY: &str = include_str!("fixtures/before-axes/desktop.conf.mix");
const BACKUP: &str = include_str!("fixtures/before-axes/desktop.previous.conf.mix");

fn binding() -> Binding {
    Binding {
        instance: "fixture".into(),
        profile: "default".into(),
    }
}

fn profile(primary: &str, backup: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in [
        ("desktop.conf.mix", primary),
        ("desktop.previous.conf.mix", backup),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    dir
}

fn format_of(dir: &std::path::Path, name: &str) -> Option<f64> {
    let value: serde_json::Value =
        strict::from_str(&std::fs::read_to_string(dir.join(name)).unwrap()).unwrap();
    value["format"].as_f64()
}

/// `text` with the stored scheme changed and its seal left as it was.
fn tampered(text: &str) -> String {
    text.replacen("\"scheme\": \"forest\"", "\"scheme\": \"ocean\"", 1)
}

#[test]
fn the_fixtures_are_format_one() {
    assert!(
        !PRIMARY.contains("\"format\""),
        "format 1 has no format field"
    );
    assert_eq!(FORMAT, 2);
}

#[test]
fn an_old_primary_and_backup_upgrade_on_open_and_accept_changes() {
    let dir = profile(PRIMARY, BACKUP);
    let (store, data) = Store::open(dir.path(), &binding()).unwrap();
    assert!(!store.restored, "the primary verified as stored");
    assert_eq!(data.revision, Revision(5), "nothing authored changed");
    assert_eq!(
        (
            data.desktop.appearance.scheme.as_str(),
            data.desktop.appearance.mode.as_str()
        ),
        ("forest", "dark")
    );
    assert_eq!(
        data.desktop.appearance.style, None,
        "the new keys take their defaults"
    );
    assert_eq!(data.format, FORMAT);
    assert_eq!(
        format_of(dir.path(), "desktop.conf.mix"),
        Some(f64::from(FORMAT)),
        "the primary was rewritten"
    );
    assert_eq!(
        format_of(dir.path(), "desktop.previous.conf.mix"),
        Some(f64::from(FORMAT)),
        "and its backup"
    );
    data.effective()
        .expect("the re-derived effective digest verifies");
    let mut authority = Authority::new(store, data).unwrap();
    let request = ApplyRequest {
        binding: binding(),
        expected_incarnation: authority.accepted.incarnation.clone(),
        expected_revision: authority.accepted.revision,
        operation_id: "after-axes".into(),
        changes: BTreeMap::from([("appearance.style".into(), json!("classic"))]),
        reset: vec![],
        request_digest: None,
    };
    assert_eq!(
        authority.apply(request).unwrap()["receipt"]["revision"],
        "6"
    );
    drop(authority);
    let (_, reopened) = Store::open(dir.path(), &binding()).unwrap();
    assert_eq!(
        reopened.desktop.appearance.style.as_deref(),
        Some("classic")
    );
}

#[test]
fn an_old_backup_restores_upgraded() {
    let dir = profile("{ incomplete", BACKUP);
    let (store, data) = Store::open(dir.path(), &binding()).unwrap();
    assert!(store.restored, "the backup restored the corrupt primary");
    assert_eq!(data.desktop.appearance.scheme, "forest");
    assert_eq!(data.format, FORMAT);
    assert_eq!(
        format_of(dir.path(), "desktop.conf.mix"),
        Some(f64::from(FORMAT))
    );
    data.effective()
        .expect("the restored effective digest verifies");
}

#[test]
fn a_tampered_old_profile_is_still_refused() {
    // Both copies tampered: nothing verifies, nothing is upgraded.
    let dir = profile(&tampered(PRIMARY), &tampered(BACKUP));
    assert!(Store::open(dir.path(), &binding()).is_err());
    assert!(
        format_of(dir.path(), "desktop.conf.mix").is_none(),
        "the tampered primary is not rewritten"
    );
    // A tampered primary alone restores from the backup; the tampered value
    // is never adopted.
    let dir = profile(&tampered(PRIMARY), BACKUP);
    let (store, data) = Store::open(dir.path(), &binding()).unwrap();
    assert!(store.restored);
    assert_eq!(data.desktop.appearance.scheme, "forest");
}

#[test]
fn a_current_record_relabelled_older_is_refused() {
    let dir = profile(PRIMARY, BACKUP);
    drop(Store::open(dir.path(), &binding()).unwrap());
    let path = dir.path().join("desktop.conf.mix");
    let text = std::fs::read_to_string(&path).unwrap();
    let relabelled = text.replacen(&format!("\"format\": {FORMAT},\n"), "", 1);
    assert_ne!(relabelled, text, "the format field was removed");
    std::fs::write(&path, relabelled).unwrap();
    std::fs::write(dir.path().join("desktop.previous.conf.mix"), "{ incomplete").unwrap();
    assert!(
        Store::open(dir.path(), &binding()).is_err(),
        "the seal covers the format"
    );
}

#[test]
fn a_second_open_changes_nothing() {
    let dir = profile(PRIMARY, BACKUP);
    drop(Store::open(dir.path(), &binding()).unwrap());
    let read = |name: &str| std::fs::read(dir.path().join(name)).unwrap();
    let (primary, backup) = (read("desktop.conf.mix"), read("desktop.previous.conf.mix"));
    let (_, data) = Store::open(dir.path(), &binding()).unwrap();
    assert_eq!(data.format, FORMAT);
    assert_eq!(
        read("desktop.conf.mix"),
        primary,
        "the upgrade is idempotent"
    );
    assert_eq!(read("desktop.previous.conf.mix"), backup);
}

#[test]
fn defaults_serialise_as_before_and_chosen_values_appear() {
    let fixture: serde_json::Value = strict::from_str(PRIMARY).unwrap();
    let appearance = &fixture["desktop"]["appearance"];
    let mut desktop = Desktop::default();
    desktop.appearance.scheme = "forest".into();
    let written = serde_json::to_value(&desktop.appearance).unwrap();
    assert_eq!(
        written.as_object().unwrap().keys().collect::<Vec<_>>(),
        appearance.as_object().unwrap().keys().collect::<Vec<_>>(),
        "the same keys as the earlier code wrote"
    );
    desktop.appearance.decorations = "ssd".into();
    assert_eq!(
        serde_json::to_value(&desktop.appearance).unwrap()["decorations"],
        "ssd"
    );
}
