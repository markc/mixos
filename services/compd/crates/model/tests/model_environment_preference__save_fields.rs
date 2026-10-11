// SPDX-License-Identifier: MIT OR Apache-2.0
//! `save_fields`: only the named keys change; everything else in the file,
//! unknown keys included, survives; a broken file is refused, not replaced.
use model::environment::preference::base::{Preference, save_fields_at};
use serde_json::{Map, Value, json};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("compd-save-fields-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("preferences.json")
}

fn fields(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

#[test]
fn merges_named_keys_and_keeps_the_rest() {
    let p = scratch("merge");
    std::fs::write(
        &p,
        r#"{"cursor_sensitivity": 2.5, "someone_elses_key": {"a": 1}, "workspaces_count": 4}"#,
    )
    .unwrap();
    save_fields_at(
        &p,
        fields(json!({"workspaces_count": 6, "workspace_names": ["Web", ""]})),
    )
    .unwrap();
    let back: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(back["cursor_sensitivity"], json!(2.5));
    assert_eq!(back["someone_elses_key"], json!({"a": 1}));
    assert_eq!(back["workspaces_count"], json!(6));
    assert_eq!(back["workspace_names"], json!(["Web", ""]));
    let prefs: Preference = serde_json::from_value(back).unwrap();
    assert_eq!(prefs.workspaces_count, 6);
    assert_eq!(prefs.workspace_names, ["Web", ""]);
    assert_eq!(leftover_tmp(&p), 0);
}

fn leftover_tmp(p: &std::path::Path) -> usize {
    std::fs::read_dir(p.parent().unwrap())
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        })
        .count()
}

/// Concurrent writers (separate opens, so separate locks) never lose each
/// other's keys or leave a torn file.
#[test]
fn concurrent_writers_keep_every_key() {
    let p = scratch("concurrent");
    std::fs::write(&p, "{}").unwrap();
    let threads: Vec<_> = (0..8)
        .map(|writer| {
            let p = p.clone();
            std::thread::spawn(move || {
                for round in 0..25 {
                    save_fields_at(&p, fields(json!({ format!("writer_{writer}"): round })))
                        .unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let back: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    for writer in 0..8 {
        assert_eq!(back[format!("writer_{writer}")], json!(24), "{back}");
    }
    assert_eq!(leftover_tmp(&p), 0);
}

#[test]
fn a_missing_file_starts_empty() {
    let p = scratch("missing");
    save_fields_at(&p, fields(json!({"workspaces_count": 2}))).unwrap();
    let back: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(back, json!({"workspaces_count": 2}));
}

#[test]
fn a_broken_file_is_refused_and_kept() {
    for (name, raw) in [("syntax", "{not json"), ("array", "[1, 2]")] {
        let p = scratch(name);
        std::fs::write(&p, raw).unwrap();
        assert!(save_fields_at(&p, fields(json!({"workspaces_count": 2}))).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), raw);
    }
}

/// `Preference::save` merges every known field over the file, so a field the
/// user cleared must still serialise (null / []) or the old value would
/// survive the merge and come back on the next load.
#[test]
fn a_cleared_field_overwrites_the_old_value() {
    let p = scratch("cleared");
    std::fs::write(
        &p,
        r#"{"ime": {"exec": "fcitx5"}, "background_shader": "x.wgsl",
            "outputs_layout": [{"id": 1}], "outputs_default_mode": {"width": 1, "height": 1, "refresh_mhz": 60000}}"#,
    )
    .unwrap();
    let cleared = serde_json::to_value(Preference::default()).unwrap();
    for key in [
        "ime",
        "background_shader",
        "outputs_layout",
        "outputs_default_mode",
    ] {
        assert!(cleared.get(key).is_some(), "{key} must always serialise");
    }
    save_fields_at(&p, cleared.as_object().unwrap().clone()).unwrap();
    let back: Preference = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert!(back.ime.is_none());
    assert!(back.background_shader.is_none());
    assert!(back.outputs_layout.is_empty());
    assert!(back.outputs_default_mode.is_none());
}

#[test]
fn defaults_without_the_keys() {
    let prefs: Preference = serde_json::from_str("{}").unwrap();
    assert_eq!(prefs.workspaces_count, 4);
    assert!(prefs.workspace_names.is_empty());
}
