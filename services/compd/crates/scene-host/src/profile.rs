// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The one seam between an accepted settings snapshot and the key space v2
//! views compd reads: `window.*` ([`Window`]), `shell.panels.*`
//! ([`Panels`]), and the `appearance.*` and `accessibility.*` choices the
//! shell look takes from them ([`Appearance`], [`Accessibility`]), decoded
//! from the authored values snapshot schema 3 carries. The look still reads
//! the resolved scheme, mode, contrast, icon stroke and design projection
//! from the snapshot's `effective`, and the font overlay from its format-4
//! appearance (`appearance::theme_of`). Writes ([`panel_changes`]) are v2 keys: the v2 store refuses v1
//! names.
use serde_json::{Value, json};
use settings::registry;
use settings::view::{Accessibility, Appearance, Panels, View, Window};
use settings::{Diagnostic, Snapshot};

/// The v2 keys compd reads, decoded once per accepted snapshot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Profile {
    pub window: Window,
    pub panels: Panels,
    pub appearance: Appearance,
    pub accessibility: Accessibility,
}

impl Profile {
    /// Read and validate the snapshot's values: a value the registry
    /// refuses (an unknown edge or mode, two enabled panels on one edge)
    /// refuses the whole presentation at that key's path.
    pub fn read(snapshot: &Snapshot) -> Result<Self, Diagnostic> {
        let values = &snapshot.values;
        registry::validate(values).map_err(|diagnostics| {
            let first = diagnostics.into_iter().next().unwrap_or_else(|| {
                Diagnostic::new("invalid_value", "", "settings values are invalid")
            });
            Diagnostic::new("unsupported_presentation", &first.path, first.message)
        })?;
        Ok(Self {
            window: Window::of(values),
            panels: Panels::of(values),
            appearance: Appearance::of(values),
            accessibility: Accessibility::of(values),
        })
    }
}

/// The `settings.apply` changes for `change` to panel `id`, in v2 keys
/// (§5.2): autohide alone keeps the mode; a mode also clears autohide (an
/// explicit dock or overlay shows the panel); a thickness is snapped to a
/// step. `Err` is a refusal body: `SETTINGS_INVALID` (with the key) when the
/// applied profile does not read, `SETTINGS_UNAVAILABLE` when it has no
/// enabled panel `id`.
pub(crate) fn panel_changes(
    snapshot: &Snapshot,
    id: &str,
    change: crate::panels::RecordChange,
) -> Result<Value, Value> {
    use crate::panels::RecordChange;
    let profile = Profile::read(snapshot).map_err(|fault| {
        json!({"error_code":"SETTINGS_INVALID", "path":fault.path,
            "message":format!("the applied settings do not read: {}", fault.message)})
    })?;
    if !profile.panels.get(id).is_some_and(|panel| panel.enabled) {
        return Err(json!({"error_code":"SETTINGS_UNAVAILABLE",
            "message":format!("the applied settings have no enabled shell.panels.{id} panel")}));
    }
    let key = |leaf: &str| format!("shell.panels.{id}.{leaf}");
    let mut changes = serde_json::Map::new();
    match change {
        RecordChange::Autohide(autohide) => {
            changes.insert(key("autohide"), json!(autohide));
        }
        RecordChange::Mode(mode) => {
            changes.insert(key("mode"), json!(mode.name()));
            changes.insert(key("autohide"), json!(false));
        }
        RecordChange::Thickness(px) => {
            changes.insert(key("thickness"), json!(registry::snap_panel_thickness(px)));
        }
    }
    Ok(Value::Object(changes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(desktop: settings::Desktop) -> Snapshot {
        crate::appearance::snapshot_of(desktop)
    }

    #[test]
    fn thickness_snaps_to_the_nearest_step_with_ties_up() {
        for (px, step) in [
            (16, 32),
            (32, 32),
            (36, 40),
            (40, 40),
            (48, 56),
            (47, 40),
            (60, 64),
            (72, 80),
            (100, 80),
            (256, 80),
        ] {
            assert_eq!(registry::snap_panel_thickness(px), step, "{px}");
        }
    }

    #[test]
    fn the_default_record_reads_as_the_v2_defaults() {
        let profile = Profile::read(&snapshot(settings::Desktop::default())).unwrap();
        let bottom = &profile.panels.bottom;
        assert!(bottom.enabled);
        assert_eq!(
            (bottom.edge.as_str(), bottom.mode.as_str()),
            ("bottom", "dock")
        );
        assert!(!bottom.autohide);
        assert_eq!(bottom.thickness, 40);
        assert!(!profile.panels.top.enabled);
        assert_eq!(profile.window.gaps, 0);
        assert_eq!(profile.window.framing.style, "mac");
        assert_eq!(profile.window.framing.decorations, "auto");
    }

    #[test]
    fn hidden_is_dock_with_autohide_and_framing_moves_to_window() {
        let mut desktop = settings::Desktop::default();
        desktop.appearance.window_style = "win11".into();
        desktop.appearance.caption_side = "left".into();
        let bottom = desktop.shell.panels.get_mut("bottom").unwrap();
        bottom.mode = "hidden".into();
        bottom.thickness = 70;
        desktop.shell.page_order.push("retired".into());
        let profile = Profile::read(&snapshot(desktop)).unwrap();
        assert_eq!(profile.panels.bottom.mode, "dock");
        assert!(profile.panels.bottom.autohide);
        assert_eq!(profile.panels.bottom.thickness, 64);
        assert_eq!(profile.window.framing.style, "win11");
        assert_eq!(profile.window.framing.caption_side, "left");
    }

    #[test]
    fn keys_only_v2_has_are_read_from_the_values() {
        let mut snapshot = snapshot(settings::Desktop::default());
        snapshot.values.insert("window.gaps".into(), json!(12));
        snapshot
            .values
            .insert("shell.panels.bottom.autohide".into(), json!(true));
        let profile = Profile::read(&snapshot).unwrap();
        assert_eq!(profile.window.gaps, 12);
        assert!(profile.panels.bottom.autohide);
        snapshot.values.insert("window.gaps".into(), json!(-1));
        assert_eq!(
            Profile::read(&snapshot).unwrap_err().path,
            "window.gaps",
            "a value the registry refuses refuses the presentation"
        );
    }

    #[test]
    fn panel_writes_are_v2_keys() {
        use crate::panels::{RecordChange, StoredMode};
        let defaults = snapshot(settings::Desktop::default());
        let change = |change| panel_changes(&defaults, "bottom", change).unwrap();
        assert_eq!(
            change(RecordChange::Autohide(true)),
            json!({"shell.panels.bottom.autohide": true})
        );
        assert_eq!(
            change(RecordChange::Mode(StoredMode::Overlay)),
            json!({"shell.panels.bottom.mode": "overlay", "shell.panels.bottom.autohide": false})
        );
        assert_eq!(
            change(RecordChange::Thickness(48)),
            json!({"shell.panels.bottom.thickness": 56})
        );
        for id in ["top", "left"] {
            assert_eq!(
                panel_changes(&defaults, id, RecordChange::Autohide(true)).unwrap_err()["error_code"],
                "SETTINGS_UNAVAILABLE"
            );
        }
        // A profile that no longer reads says so, with its key.
        let mut broken = snapshot(settings::Desktop::default());
        broken
            .values
            .insert("shell.panels.bottom.edge".into(), json!("leftish"));
        let fault = panel_changes(&broken, "bottom", RecordChange::Autohide(true)).unwrap_err();
        assert_eq!(fault["error_code"], "SETTINGS_INVALID");
        assert_eq!(fault["path"], "shell.panels.bottom.edge");
    }
}
