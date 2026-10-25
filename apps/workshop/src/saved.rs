// SPDX-License-Identifier: MIT OR Apache-2.0
//! Workshop's app-owned arrangement. Frame and widget state belong to toolkit.
use inspector::{DockSide, Engine, Row, Selection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
impl Arrangement {
    pub fn capture(engine: &Engine) -> Self {
        Self { selected: engine.ui.selected.clone(), row_key: engine.ui.row_key.clone(),
            expanded: engine.ui.expanded.clone(), filter: engine.ui.filter.clone(),
            macros_open: engine.ui.macros_open, dock_side: engine.ui.dock_side,
            dock_open: engine.workshop.dock_open, paused: engine.workshop.paused,
            traffic_filter: engine.workshop.filter.clone() }
    }
    /// Apply only passive state: never call `select_row`, run a macro or issue
    /// a Bus call as a side effect of restoring a selection.
    pub fn apply(&self, engine: &mut Engine) {
        engine.ui.expanded = self.expanded.clone(); engine.ui.filter = self.filter.clone();
        engine.ui.macros_open = self.macros_open; engine.ui.dock_side = self.dock_side;
        engine.workshop.dock_open = self.dock_open; engine.workshop.paused = self.paused;
        engine.workshop.filter = self.traffic_filter.clone();
    }
}

/// Selection waits for live discovery. Expanded keys are installed up front;
/// after discovery finishes, only keys seen in the live tree survive saving.
pub struct Pending {
    selected: Option<Selection>, row_key: Option<String>,
    pub show_selection: bool,
    done: bool,
}
impl Pending {
    pub fn new(saved: &Arrangement) -> Self {
        Self { selected: saved.selected.clone(), row_key: saved.row_key.clone(), show_selection: false, done: false }
    }
    pub fn reconcile(&mut self, engine: &mut Engine) {
        if self.done { return; }
        let mut keys = BTreeSet::new();
        collect(&engine.tree(), &mut keys);
        // A user/Bus selection made during discovery wins over the old one.
        if engine.ui.row_key.is_some() { self.row_key = None; self.selected = None; }
        if self.row_key.as_ref().is_some_and(|k| keys.contains(k)) {
            engine.ui.row_key = self.row_key.take();
            engine.ui.selected = self.selected.take().filter(|s| engine.snapshot.command_name(s).is_some());
            self.show_selection = true;
        }
        if engine.snapshot.error.is_none()
            && engine.snapshot.progress.as_ref().is_some_and(|p| p.finished) {
            engine.ui.expanded.retain(|k| keys.contains(k));
            self.row_key = None; self.selected = None; self.done = true;
        }
    }
    pub fn ready(&self) -> bool { self.done || self.show_selection }
}
fn collect(rows: &[Row], keys: &mut BTreeSet<String>) {
    for row in rows { keys.insert(row.key.clone()); collect(&row.children, keys); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arrangement_restores_only_the_listed_fields() {
        let mut e = Engine::new(crate::label);
        e.ui.selected = Some(Selection { service: "example".into(), command: "get".into() });
        e.ui.row_key = Some("command:example:get".into());
        e.ui.expanded.insert("service:example".into()); e.ui.filter = "get".into();
        e.ui.macros_open = true; e.ui.dock_side = DockSide::Right;
        e.workshop.dock_open = true; e.workshop.paused = true; e.workshop.filter = "query".into();
        e.ui.body = "unsaved".into(); e.ui.focus_filter = true; e.ui.dialog = Some(inspector::Dialog::About);
        let a = Arrangement::capture(&e);
        let raw = serde_json::to_string(&a).unwrap();
        for field in ["body", "dialog", "focus_filter", "reply", "rows"] { assert!(!raw.contains(field)); }
        let back: Arrangement = serde_json::from_str(&raw).unwrap(); assert_eq!(a, back);
        let mut restored = Engine::new(crate::label); back.apply(&mut restored);
        assert_eq!(restored.ui.filter, "get"); assert!(restored.ui.macros_open);
        assert_eq!(restored.ui.dock_side, DockSide::Right); assert!(restored.workshop.dock_open && restored.workshop.paused);
        assert_eq!(restored.workshop.filter, "query"); assert!(restored.ui.body.is_empty());
        assert!(restored.ui.dialog.is_none() && !restored.ui.focus_filter && restored.reply.is_empty());
    }
}
