// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The one seam between an accepted settings snapshot and the key space v2
//! views compd reads: `window.*` ([`Window`]) and `shell.panels.*`
//! ([`Panels`]), decoded from the authored values snapshot schema 3
//! carries. Writes ([`panel_changes`]) are v2 keys: the v2 store refuses v1
//! names.
use serde_json::{Value, json};
use settings::registry;
use settings::view::{Panels, View, Window};
use settings::{Diagnostic, Snapshot};

/// The panel thickness steps, in pixels (§2.4, F9).
pub(crate) const THICKNESS_STEPS: [u32; 5] = [32, 40, 56, 64, 80];

/// The v2 keys compd reads, decoded once per accepted snapshot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Profile {
    pub window: Window,
    pub panels: Panels,
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
        })
    }
}

/// The nearest thickness step, ties up (F9).
pub(crate) fn snap_thickness(px: u32) -> u32 {
    let mut best = THICKNESS_STEPS[0];
    for step in THICKNESS_STEPS {
        if step.abs_diff(px) <= best.abs_diff(px) {
            best = step;
        }
    }
    best
}

/// The `settings.apply` changes for `change` to panel `id`, in v2 keys
/// (§5.2): autohide alone keeps the mode; a mode also clears autohide (an
/// explicit dock or overlay shows the panel); a thickness is snapped to a
/// step. `None` when the applied profile has no enabled panel `id`.
pub(crate) fn panel_changes(
    snapshot: &Snapshot,
    id: &str,
    change: crate::panels::RecordChange,
) -> Option<Value> {
    use crate::panels::RecordChange;
    let profile = Profile::read(snapshot).ok()?;
    if !profile.panels.get(id)?.enabled {
        return None;
    }
    let key = |leaf: &str| format!("shell.panels.{id}.{leaf}");
    let mut changes = serde_json::Map::new();
    match change {
        RecordChange::Autohide(autohide) => {
            changes.insert(key("autohide"), json!(autohide));
        }
        RecordChange::Mode(mode) => {
            changes.insert(key("mode"), json!(mode));
            changes.insert(key("autohide"), json!(false));
        }
        RecordChange::Thickness(px) => {
            changes.insert(key("thickness"), json!(snap_thickness(px)));
        }
    }
    Some(Value::Object(changes))
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
            assert_eq!(snap_thickness(px), step, "{px}");
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
        use crate::panels::RecordChange;
        let snapshot = snapshot(settings::Desktop::default());
        let change = |change| panel_changes(&snapshot, "bottom", change).unwrap();
        assert_eq!(
            change(RecordChange::Autohide(true)),
            json!({"shell.panels.bottom.autohide": true})
        );
        assert_eq!(
            change(RecordChange::Mode("overlay")),
            json!({"shell.panels.bottom.mode": "overlay", "shell.panels.bottom.autohide": false})
        );
        assert_eq!(
            change(RecordChange::Thickness(48)),
            json!({"shell.panels.bottom.thickness": 56})
        );
        assert!(panel_changes(&snapshot, "top", RecordChange::Autohide(true)).is_none());
        assert!(panel_changes(&snapshot, "left", RecordChange::Autohide(true)).is_none());
    }
}
