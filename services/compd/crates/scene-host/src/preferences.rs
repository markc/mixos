// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Prepare the global profile once; each output fits it in its existing model.
//!
//! Each enabled `shell.panels.<panel>` (`top`, `bottom`) owns its edge: `dock`
//! reserves it, `overlay` pins it over the windows, and `autohide` (with
//! either mode) conceals it until summoned. A disabled panel leaves its edge
//! to Quoin's own model.
use edges::{Edge, PanelMode, PanelPreference};
use settings::Diagnostic;
use settings::view::{Panel, Panels};

#[derive(Clone, Debug, Default)]
pub(crate) struct Preferences {
    records: [Option<Record>; 4],
}

#[derive(Clone, Debug)]
pub(crate) struct Record {
    pub id: String,
    pub preference: PanelPreference,
}

fn unsupported(path: &str, message: &str) -> Diagnostic {
    Diagnostic::new("unsupported_presentation", path, message)
}

impl Preferences {
    pub fn prepare(panels: &Panels) -> Result<Self, Diagnostic> {
        let mut policy = Self::default();
        for (id, panel) in [("top", &panels.top), ("bottom", &panels.bottom)] {
            if panel.enabled {
                policy.add(id, panel)?;
            }
        }
        Ok(policy)
    }

    fn add(&mut self, id: &str, panel: &Panel) -> Result<(), Diagnostic> {
        let path = |leaf: &str| format!("shell.panels.{id}.{leaf}");
        let edge = match panel.edge.as_str() {
            "left" => Edge::Left,
            "bottom" => Edge::Bottom,
            "right" => Edge::Right,
            "top" => Edge::Top,
            _ => return Err(unsupported(&path("edge"), "Unknown panel edge")),
        };
        if self.records[edge.index()].is_some() {
            // panel_edges: the registry refuses it first; this is the second line.
            return Err(unsupported(
                &path("edge"),
                "Two enabled panels target the same edge",
            ));
        }
        let mode = match (panel.mode.as_str(), panel.autohide) {
            ("dock" | "overlay", true) => PanelMode::Hidden,
            ("dock", false) => PanelMode::Docked,
            ("overlay", false) => PanelMode::Pinned,
            _ => return Err(unsupported(&path("mode"), "Unknown panel mode")),
        };
        if !crate::profile::THICKNESS_STEPS.contains(&panel.thickness) {
            return Err(unsupported(
                &path("thickness"),
                "Invalid requested thickness",
            ));
        }
        let preference = PanelPreference::new(mode, panel.thickness as f32)
            .map_err(|error| unsupported(&path("thickness"), &error.to_string()))?;
        self.records[edge.index()] = Some(Record {
            id: id.into(),
            preference,
        });
        Ok(())
    }

    pub fn values(&self) -> [Option<PanelPreference>; 4] {
        std::array::from_fn(|index| self.records[index].as_ref().map(|record| record.preference))
    }

    pub fn record(&self, edge: Edge) -> Option<&Record> {
        self.records[edge.index()].as_ref()
    }
}

/// Typed panels from `(path, value)` pairs over the registry defaults.
#[cfg(test)]
pub(crate) fn panels_of(pairs: &[(&str, serde_json::Value)]) -> Panels {
    use settings::view::View;
    let values: settings::registry::Values = pairs
        .iter()
        .map(|(path, value)| ((*path).to_owned(), value.clone()))
        .collect();
    Panels::of(&values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn both_panels_map_to_their_edges_and_modes() {
        for (edge, name, mode, autohide, expected) in [
            (Edge::Top, "top", "dock", false, PanelMode::Docked),
            (Edge::Left, "left", "overlay", false, PanelMode::Pinned),
            (Edge::Right, "right", "dock", true, PanelMode::Hidden),
            (Edge::Top, "top", "overlay", true, PanelMode::Hidden),
        ] {
            let policy = Preferences::prepare(&panels_of(&[
                ("shell.panels.top.enabled", json!(true)),
                ("shell.panels.top.edge", json!(name)),
                ("shell.panels.top.mode", json!(mode)),
                ("shell.panels.top.autohide", json!(autohide)),
                ("shell.panels.top.thickness", json!(56)),
                ("shell.panels.bottom.enabled", json!(false)),
            ]))
            .unwrap();
            let record = policy.record(edge).unwrap();
            assert_eq!(record.id, "top");
            assert_eq!(record.preference.mode(), expected);
            assert_eq!(record.preference.thickness(), 56.0);
        }
    }

    #[test]
    fn disabled_panels_own_no_edge_and_defaults_own_the_bottom() {
        let policy = Preferences::prepare(&panels_of(&[])).unwrap();
        let bottom = policy.record(Edge::Bottom).unwrap();
        assert_eq!(bottom.id, "bottom");
        assert_eq!(bottom.preference.mode(), PanelMode::Docked);
        assert_eq!(bottom.preference.thickness(), 40.0);
        assert!(policy.record(Edge::Top).is_none());
        let none =
            Preferences::prepare(&panels_of(&[("shell.panels.bottom.enabled", json!(false))]))
                .unwrap();
        assert!(none.values().iter().all(Option::is_none));
    }

    #[test]
    fn enabled_panels_on_one_edge_refuse_the_whole_policy() {
        let fault = Preferences::prepare(&panels_of(&[
            ("shell.panels.top.enabled", json!(true)),
            ("shell.panels.top.edge", json!("bottom")),
        ]))
        .unwrap_err();
        assert_eq!(fault.code, "unsupported_presentation");
        assert_eq!(fault.path, "shell.panels.bottom.edge");
    }

    #[test]
    fn unknown_edge_mode_and_thickness_report_the_key_path() {
        for (leaf, value) in [
            ("edge", json!("leftish")),
            ("mode", json!("float")),
            ("mode", json!("hidden")),
            ("thickness", json!(48)),
        ] {
            let path = format!("shell.panels.bottom.{leaf}");
            let fault = Preferences::prepare(&panels_of(&[(&path, value)])).unwrap_err();
            assert_eq!(fault.code, "unsupported_presentation");
            assert_eq!(fault.path, path);
        }
    }
}
