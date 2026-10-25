// SPDX-License-Identifier: MIT OR Apache-2.0
//! Workshop's app-owned arrangement. Frame and widget state belong to toolkit.
use inspector::{DockSide, Engine, Row, Selection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Arrangement {
    pub selected: Option<Selection>,
    pub row_key: Option<String>,
    pub expanded: BTreeSet<String>,
    pub filter: String,
    pub macros_open: bool,
    pub dock_side: DockSide,
    pub dock_open: bool,
    pub paused: bool,
    pub traffic_filter: String,
}
impl Default for Arrangement {
    fn default() -> Self {
        Self {
            selected: None,
            row_key: None,
            expanded: BTreeSet::new(),
            filter: String::new(),
            macros_open: false,
            dock_side: DockSide::Auto,
            dock_open: true,
            paused: false,
            traffic_filter: String::new(),
        }
    }
}
impl Arrangement {
    pub fn capture(engine: &Engine) -> Self {
        Self {
            selected: engine.ui.selected.clone(),
            row_key: engine.ui.row_key.clone(),
            expanded: engine.ui.expanded.clone(),
            filter: engine.ui.filter.clone(),
            macros_open: engine.ui.macros_open,
            dock_side: engine.ui.dock_side,
            dock_open: engine.workshop.dock_open,
            paused: engine.workshop.paused,
            traffic_filter: engine.workshop.filter.clone(),
        }
    }
    /// Apply only passive state: never call `select_row`, run a macro or issue
    /// a Bus call as a side effect of restoring a selection.
    pub fn apply(&self, engine: &mut Engine) {
        engine.ui.expanded = self.expanded.clone();
        engine.ui.filter = self.filter.clone();
        engine.ui.macros_open = self.macros_open;
        engine.ui.dock_side = self.dock_side;
        engine.workshop.dock_open = self.dock_open;
        engine.workshop.paused = self.paused;
        engine.workshop.filter = self.traffic_filter.clone();
    }
}

/// Selection waits for live discovery. Expanded keys are installed up front;
/// after discovery finishes, only keys seen in the live tree survive saving.
pub struct Pending {
    selected: Option<Selection>,
    row_key: Option<String>,
    pub show_selection: bool,
    done: bool,
    expanded: Option<BTreeSet<String>>,
    seen: BTreeSet<String>,
}
impl Pending {
    pub fn new(saved: &Arrangement) -> Self {
        Self {
            selected: saved.selected.clone(),
            row_key: saved.row_key.clone(),
            show_selection: false,
            done: false,
            expanded: Some(saved.expanded.clone()),
            seen: BTreeSet::new(),
        }
    }
    /// First launch uses discovery's defaults, including its open local node.
    pub fn follow_defaults(&mut self) { self.expanded = None; }
    pub fn reconcile(&mut self, engine: &mut Engine) {
        if self.done {
            return;
        }
        let mut keys = BTreeSet::new();
        let filter = std::mem::take(&mut engine.ui.filter);
        collect(&engine.tree(), &mut keys);
        engine.ui.filter = filter;
        if let Some(expanded) = &self.expanded {
            for node in &engine.snapshot.nodes {
                let key = format!("node:{}", node.name);
                if node.local && !self.seen.contains(&key) && !expanded.contains(&key) {
                    engine.ui.expanded.remove(&key);
                }
            }
            for key in keys.difference(&self.seen) {
                if expanded.contains(key) { engine.ui.expanded.insert(key.clone()); }
            }
        }
        self.seen.extend(keys.iter().cloned());
        // A user/Bus selection made during discovery wins over the old one.
        if engine.ui.row_key.is_some() {
            self.row_key = None;
            self.selected = None;
        }
        if self.row_key.as_ref().is_some_and(|k| keys.contains(k)) {
            engine.ui.row_key = self.row_key.take();
            engine.ui.selected = self
                .selected
                .take()
                .filter(|s| engine.snapshot.command_name(s).is_some());
            self.show_selection = true;
        }
        if engine.snapshot.error.is_none()
            && engine.workshop.pending == 0
            && engine
                .snapshot
                .progress
                .as_ref()
                .is_some_and(|p| p.finished)
        {
            engine.ui.expanded.retain(|k| keys.contains(k));
            self.row_key = None;
            self.selected = None;
            self.done = true;
        }
    }
    pub fn ready(&self) -> bool {
        self.done || self.show_selection
    }
    pub fn preserve_pending(&self, app: &mut Arrangement) {
        if !self.done && self.row_key.is_some() && app.row_key.is_none() {
            app.row_key = self.row_key.clone();
            app.selected = self.selected.clone();
        }
    }
}
fn collect(rows: &[Row], keys: &mut BTreeSet<String>) {
    for row in rows {
        keys.insert(row.key.clone());
        collect(&row.children, keys);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arrangement_restores_only_the_listed_fields() {
        let mut e = Engine::new(crate::label);
        e.ui.selected = Some(Selection {
            service: "example".into(),
            command: "get".into(),
        });
        e.ui.row_key = Some("command:example:get".into());
        e.ui.expanded.insert("service:example".into());
        e.ui.filter = "get".into();
        e.ui.macros_open = true;
        e.ui.dock_side = DockSide::Right;
        e.workshop.dock_open = true;
        e.workshop.paused = true;
        e.workshop.filter = "query".into();
        e.ui.body = "unsaved".into();
        e.ui.focus_filter = true;
        e.ui.dialog = Some(inspector::Dialog::About);
        let a = Arrangement::capture(&e);
        let raw = serde_json::to_string(&a).unwrap();
        for field in ["body", "dialog", "focus_filter", "reply", "rows"] {
            assert!(!raw.contains(field));
        }
        let back: Arrangement = serde_json::from_str(&raw).unwrap();
        assert_eq!(a, back);
        let mut restored = Engine::new(crate::label);
        back.apply(&mut restored);
        assert_eq!(restored.ui.filter, "get");
        assert!(restored.ui.macros_open);
        assert_eq!(restored.ui.dock_side, DockSide::Right);
        assert!(restored.workshop.dock_open && restored.workshop.paused);
        assert_eq!(restored.workshop.filter, "query");
        assert!(restored.ui.body.is_empty());
        assert!(
            restored.ui.dialog.is_none() && !restored.ui.focus_filter && restored.reply.is_empty()
        );
    }
    #[test]
    fn late_rows_restore_passively_and_missing_keys_drop_after_discovery() {
        let a = Arrangement {
            selected: Some(Selection {
                service: "example".into(),
                command: "get".into(),
            }),
            row_key: Some("command:example:get".into()),
            expanded: ["service:example".into(), "service:never".into()].into(),
            filter: "does not match anything".into(),
            ..Arrangement::default()
        };
        let mut e = Engine::new(crate::label);
        e.take_effects();
        a.apply(&mut e);
        let mut pending = Pending::new(&a);
        pending.reconcile(&mut e);
        assert!(e.ui.selected.is_none());
        assert!(e.ui.expanded.contains("service:never"));
        let mut saved = Arrangement::capture(&e);
        pending.preserve_pending(&mut saved);
        assert_eq!(saved.selected, a.selected);
        e.snapshot.services.insert(
            "example".into(),
            Ok(vec![inspector::model::Command {
                name: "get".into(),
                args: String::new(),
                description: String::new(),
                read_only: Some(true),
            }]),
        );
        e.snapshot.progress = Some(inspector::model::Progress {
            total: 2,
            completed: 1,
            finished: false,
        });
        pending.reconcile(&mut e);
        assert_eq!(e.ui.selected, a.selected);
        assert_eq!(e.ui.row_key, a.row_key);
        assert!(pending.show_selection);
        assert!(
            e.take_effects().is_empty(),
            "restoration never runs a command"
        );
        e.snapshot.progress.as_mut().unwrap().finished = true;
        pending.reconcile(&mut e);
        assert!(!e.ui.expanded.contains("service:never"));
        assert!(e.ui.expanded.contains("service:example"));
        assert_eq!(
            e.ui.filter, a.filter,
            "filtering must not prune hidden live keys"
        );
        let mut missing = Pending::new(&Arrangement {
            row_key: Some("never".into()),
            ..Arrangement::default()
        });
        let mut e = Engine::new(crate::label);
        e.snapshot.progress = Some(inspector::model::Progress {
            finished: true,
            ..Default::default()
        });
        missing.reconcile(&mut e);
        let mut saved = Arrangement::capture(&e);
        missing.preserve_pending(&mut saved);
        assert!(saved.row_key.is_none());
    }
}
