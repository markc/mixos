// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lab compound-view state. Logical indexes are valid only within a generation.
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{Color32, FontId, Galley, Ui};
use serde_json::{Value, json};

pub const ROW_BUDGET: usize = 768;
pub const TEXT_BUDGET: usize = 1024;
pub const PAGE: usize = 128;

/// Scene bindings normalise JSON numbers to f64, including nested objects.
/// Only exact, nonnegative integers within Mix's safe integer range count.
pub(crate) trait TableInteger {
    fn whole(&self) -> Option<u64>;
}
impl TableInteger for Value {
    fn whole(&self) -> Option<u64> {
        self.as_f64()
            .filter(|n| {
                n.is_finite() && *n >= 0.0 && *n <= 9_007_199_254_740_991.0 && n.fract() == 0.0
            })
            .map(|n| n as u64)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection(pub Vec<(usize, usize)>);

impl Selection {
    pub fn contains(&self, index: usize) -> bool {
        self.0.iter().any(|&(a, b)| a <= index && index < b)
    }

    pub fn range(&mut self, a: usize, b: usize) {
        self.0 = if a < b { vec![(a, b)] } else { Vec::new() };
    }

    pub fn toggle(&mut self, index: usize) {
        if self.contains(index) {
            let mut out = Vec::new();
            for &(a, b) in &self.0 {
                if index < a || index >= b {
                    out.push((a, b));
                } else {
                    if a < index {
                        out.push((a, index));
                    }
                    if index + 1 < b {
                        out.push((index + 1, b));
                    }
                }
            }
            self.0 = out;
        } else {
            self.0.push((index, index + 1));
            self.0.sort_unstable();
            let mut merged: Vec<(usize, usize)> = Vec::new();
            for &(a, b) in &self.0 {
                if let Some(last) = merged.last_mut()
                    && a <= last.1
                {
                    last.1 = last.1.max(b);
                } else {
                    merged.push((a, b));
                }
            }
            self.0 = merged;
        }
    }

    pub fn count(&self) -> usize {
        self.0.iter().map(|(a, b)| b - a).sum()
    }
}

#[derive(Debug)]
pub struct Row {
    pub id: String,
    pub cells: Vec<String>,
    pub edit: String,
    pub raw: Value,
}

#[derive(Debug)]
pub struct Edit {
    pub index: usize,
    pub id: String,
    pub text: String,
    pub pending: Option<u64>,
    pub focus: bool,
}

#[derive(Debug)]
struct Text {
    text: String,
    font: FontId,
    epoch: u64,
    used: u64,
    galley: Arc<Galley>,
}

#[derive(Debug, Default)]
pub struct State {
    pub(super) dataset: Option<Arc<super::dataset::Dataset>>,
    source: Value,
    failed_revision: Option<(u64, u64)>,
    pub model_error: Option<String>,
    pub model_error_sent: bool,
    pub model_owner: String,
    pub model_load_us: u64,
    pub model_loads: u64,
    pub discard_scroll_tail: bool,
    pub session: String,
    pub generation: u64,
    pub revision: u64,
    pub total: usize,
    pub rows: BTreeMap<usize, Arc<Row>>,
    pub selection: Selection,
    pub focus: Option<usize>,
    pub anchor: Option<usize>,
    pub seq: u64,
    selection_epoch: u64,
    pub edit: Option<Edit>,
    pub first: usize,
    pub end: usize,
    pub scroll_y: f32,
    pub scroll_to: Option<usize>,
    pub restore_scroll: Option<f32>,
    pub old_active: bool,
    pub request: Option<usize>,
    pub stale: u64,
    pub refused: u64,
    pub updates: u64,
    pub shapes: u64,
    pub hits: u64,
    pub text_epoch: u64,
    pub disable_text_cache: bool,
    texts: BTreeMap<u64, Text>,
    clock: u64,
}

impl State {
    /// Logical state belongs to the loaded scene, not its disposable GPU/UI instance.
    pub fn remount(&mut self) {
        self.texts.clear(); // Galleys reference the previous egui context font atlas.
        self.old_active = false;
        self.restore_scroll = Some(self.scroll_y);
        self.request = None;
        if let Some(edit) = &mut self.edit {
            edit.focus = true;
        }
    }

    /// Validate the whole page before changing state. A generation is a row-order
    /// identity; changing a row's identity in place requires a new generation.
    pub fn ingest(&mut self, view: &Value) -> bool {
        let (Some(generation), Some(revision), Some(total), Some(offset), Some(rows)) = (
            view["generation"].whole(),
            view["revision"].whole(),
            view["total"].whole(),
            view["offset"].whole(),
            view["rows"].as_array(),
        ) else {
            self.refused += 1;
            return false;
        };
        if self.failed_revision == Some((generation, revision)) {
            return false;
        }
        if generation < self.generation
            || (generation == self.generation && revision < self.revision)
        {
            self.stale += 1;
            return false;
        }
        if generation == self.generation && revision == self.revision {
            return false;
        }
        if generation == 0
            || total > 1_000_000
            || offset > total
            || rows.len() > 256
            || offset + rows.len() as u64 > total
        {
            self.refused += 1;
            return false;
        }
        let mut next = Vec::new();
        let mut ids = std::collections::BTreeSet::new();
        for (i, raw) in rows.iter().enumerate() {
            let (Some(id), Some(cells)) = (raw["id"].as_str(), raw["cells"].as_array()) else {
                self.refused += 1;
                return false;
            };
            let index = offset as usize + i;
            if id.len() > 4096
                || !ids.insert(id)
                || cells.len() > 16
                || cells
                    .iter()
                    .any(|v| v.as_str().is_none_or(|s| s.len() > 4096))
                || (generation == self.generation
                    && self.rows.get(&index).is_some_and(|old| old.id != id))
            {
                self.refused += 1;
                return false;
            }
            next.push((
                index,
                Arc::new(Row {
                    id: id.to_owned(),
                    cells: cells
                        .iter()
                        .map(|v| v.as_str().unwrap().to_owned())
                        .collect(),
                    edit: raw["edit"].as_str().unwrap_or("").to_owned(),
                    raw: raw.clone(),
                }),
            ));
        }
        let epoch = view["selection_epoch"].whole().unwrap_or(0);
        let reset = generation != self.generation;
        let mut incoming_selection = Selection::default();
        if let Some(selection) = view.get("selection") {
            let Some(ranges) = selection["ranges"].as_array() else {
                self.refused += 1;
                return false;
            };
            let mut end = 0;
            for range in ranges {
                let (Some(a), Some(b)) = (range[0].whole(), range[1].whole()) else {
                    self.refused += 1;
                    return false;
                };
                if a >= b || b > total || a < end {
                    self.refused += 1;
                    return false;
                }
                incoming_selection.0.push((a as usize, b as usize));
                end = b;
            }
        }
        if self.dataset.is_some() && !reset && view.get("source") != Some(&self.source) {
            self.refused += 1;
            return false;
        }
        let dataset = if let Some(source) = view.get("source").filter(|v| v.is_object()) {
            if !reset && self.dataset.is_some() && source == &self.source {
                self.dataset.clone()
            } else {
                let started = std::time::Instant::now();
                let loaded = super::dataset::Dataset::load(
                    source,
                    &self.session,
                    &self.model_owner,
                    generation,
                    total as usize,
                );
                self.model_load_us = started.elapsed().as_micros() as u64;
                self.model_loads += 1;
                match loaded {
                    Ok(model) => Some(model),
                    Err(error) => {
                        self.model_error = Some(error);
                        self.model_error_sent = false;
                        self.failed_revision = Some((generation, revision));
                        self.refused += 1;
                        return false;
                    }
                }
            }
        } else {
            None
        };
        if dataset
            .as_ref()
            .is_some_and(|m| m.rows.len() != total as usize)
            || (dataset.is_some() && !rows.is_empty())
        {
            self.refused += 1;
            return false;
        }
        if reset {
            self.rows.clear();
            self.selection = Selection::default();
            self.focus = None;
            self.anchor = None;
            self.edit = None;
            self.request = None;
            self.selection_epoch = 0;
            self.first = 0;
            self.end = 0;
            self.scroll_y = 0.0;
            self.restore_scroll = None;
            self.scroll_to = Some(0);
        }
        if reset || epoch > self.selection_epoch {
            self.selection_epoch = epoch;
            self.focus = view["focus"]
                .whole()
                .map(|v| v as usize)
                .filter(|&v| v < total as usize);
            self.selection = incoming_selection;
            self.anchor = self.focus;
            self.scroll_to = self.focus.or(if reset { Some(0) } else { None });
        }
        self.dataset = dataset;
        self.source = view["source"].clone();
        self.model_error = None;
        self.model_error_sent = false;
        self.failed_revision = None;
        self.generation = generation;
        self.revision = revision;
        self.total = total as usize;
        self.updates += 1;
        for (index, row) in next {
            self.rows.insert(index, row);
        }
        let pivot = self.request.unwrap_or(self.first);
        self.request = None;
        while self.rows.len() > ROW_BUDGET {
            let index = *self
                .rows
                .keys()
                .max_by_key(|&&i| i.abs_diff(pivot))
                .unwrap();
            self.rows.remove(&index);
        }
        if self
            .edit
            .as_ref()
            .and_then(|e| e.pending)
            .is_some_and(|seq| view["ack"].whole().unwrap_or(0) >= seq)
        {
            self.edit = None;
        }
        true
    }

    pub fn prepare_range(&mut self, first: usize, end: usize) {
        let Some(data) = self.dataset.clone() else {
            return;
        };
        let start = first.saturating_sub(32) / PAGE * PAGE;
        let end = (end + PAGE).min(self.total);
        for i in start..end {
            if !self.rows.contains_key(&i)
                && let Some(row) = data.row(i)
            {
                self.rows.insert(i, row);
            }
        }
        while self.rows.len() > ROW_BUDGET {
            let i = *self
                .rows
                .keys()
                .max_by_key(|&&i| i.abs_diff(first))
                .unwrap();
            self.rows.remove(&i);
        }
    }

    fn row_id(&self, index: usize) -> Option<String> {
        self.dataset
            .as_ref()
            .and_then(|m| m.id(index))
            .or_else(|| self.rows.get(&index).map(|r| r.id.clone()))
    }

    pub fn pick(&mut self, index: usize, ctrl: bool, shift: bool) {
        if index >= self.total {
            return;
        }
        if shift {
            let anchor = self.anchor.unwrap_or(index);
            self.selection
                .range(anchor.min(index), anchor.max(index) + 1);
        } else if ctrl {
            self.selection.toggle(index);
            self.anchor = Some(index);
        } else {
            self.selection.range(index, index + 1);
            self.anchor = Some(index);
        }
        self.focus = Some(index);
    }

    pub fn event(&mut self, kind: &str) -> Value {
        self.seq += 1;
        let mut selected = Vec::new();
        if self.selection.count() <= 256 {
            for &(a, b) in &self.selection.0 {
                for i in a..b {
                    if let Some(id) = self.row_id(i) {
                        selected.push(id);
                    }
                }
            }
        }
        let complete = selected.len() == self.selection.count();
        json!({"selected_ids":selected,"selection_complete":complete,"focus_id":self.focus.and_then(|i|self.row_id(i)),"kind":kind,"session":self.session,"generation":self.generation,"seq":self.seq,
            "focus":self.focus,"selection":{"ranges":self.selection.0}})
    }

    pub fn galley(
        &mut self,
        ui: &Ui,
        _index: usize,
        _col: usize,
        text: &str,
        font: FontId,
    ) -> Arc<Galley> {
        self.clock += 1;
        if self.disable_text_cache {
            self.shapes += 1;
            return ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER));
        }
        // Layout is independent of row identity and order. Verify the content
        // as well as the hash so collisions can only evict, never misrender.
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hash);
        font.size.to_bits().hash(&mut hash);
        font.family.hash(&mut hash);
        self.text_epoch.hash(&mut hash);
        let key = hash.finish();
        if let Some(cached) = self.texts.get_mut(&key)
            && cached.text == text
            && cached.font == font
            && cached.epoch == self.text_epoch
        {
            cached.used = self.clock;
            self.hits += 1;
            return Arc::clone(&cached.galley);
        }
        let galley =
            ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER));
        self.shapes += 1;
        self.texts.insert(
            key,
            Text {
                text: text.to_owned(),
                font,
                epoch: self.text_epoch,
                used: self.clock,
                galley: Arc::clone(&galley),
            },
        );
        if self.texts.len() > TEXT_BUDGET {
            let key = *self.texts.iter().min_by_key(|(_, t)| t.used).unwrap().0;
            self.texts.remove(&key);
        }
        galley
    }

    pub fn snapshot(&self) -> Value {
        json!({"model_load_us":self.model_load_us,"model_loads":self.model_loads,"native_model":self.dataset.is_some(),"model_error":self.model_error,"session":self.session,"generation":self.generation,"revision":self.revision,"seq":self.seq,
            "focus":self.focus,"focused_id":self.focus.and_then(|i| self.rows.get(&i)).map(|r| &r.id),
            "selection":{"ranges":self.selection.0,"count":self.selection.count()},
            "first":self.first,"end":self.end,"scroll_y":self.scroll_y,
            "cache_rows":self.rows.len(),"cache_cells":self.texts.len(),"shapes":self.shapes,
            "hits":self.hits,"stale":self.stale,"refused":self.refused,"updates":self.updates,
            "editing":self.edit.as_ref().map(|e| json!({"id":e.id,"text":e.text,"pending":e.pending})),
            "visible_ids":self.rows.range(self.first..self.end).map(|(_, r)| &r.id).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_cache_reuses_layout_and_invalidates_font_text_epoch_and_budget() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let font = FontId::proportional(13.0);
            state.galley(ui, 0, 0, "é 日本語 😀", font.clone());
            state.galley(ui, 0, 0, "é 日本語 😀", font.clone());
            assert_eq!((state.shapes, state.hits), (1, 1));
            state.galley(ui, 99, 2, "é 日本語 😀", font.clone());
            assert!(state.ingest(&page(1, 1, 0)));
            state.galley(ui, 0, 0, "é 日本語 😀", font.clone());
            assert_eq!((state.shapes, state.hits), (1, 3));
            state.text_epoch += 1;
            state.galley(ui, 0, 0, "é 日本語 😀", font.clone());
            state.galley(ui, 0, 0, "é 日本語 😀", FontId::proportional(15.0));
            state.galley(ui, 0, 0, "different", font.clone());
            assert_eq!(state.shapes, 4);
            state.disable_text_cache = true;
            state.galley(ui, 0, 0, "different", font.clone());
            assert_eq!(state.shapes, 5);
            state.disable_text_cache = false;
            for i in 0..TEXT_BUDGET + 20 {
                state.galley(ui, i, 0, &format!("entry-{i}"), font.clone());
            }
            assert!(state.texts.len() <= TEXT_BUDGET);
        });
        output.textures_delta.clear();
    }
    #[test]
    fn native_model_pages_are_local_bounded_and_snapshot_identity_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.json");
        let data = json!({"version":1,"token":"t","session":"s","owner":"table","generation":1,"base":"/fixture","expanded":{},
            "rows":(0..10000).map(|i|json!([format!("file-{i}"),format!("file-{i}"),i,0,1.0,false,false,0])).collect::<Vec<_>>()});
        std::fs::write(&path, data.to_string()).unwrap();
        let mut view = json!({"generation":1,"revision":1,"total":10000,"offset":0,"rows":[],"source":{"path":path,"token":"t"}});
        let mut s = State::default();
        s.session = "s".into();
        s.model_owner = "table".into();
        assert!(s.ingest(&view));
        std::fs::remove_file(&path).unwrap();
        for n in 0..50 {
            s.prepare_range(n * 128, n * 128 + 24);
            assert!(s.rows.len() <= ROW_BUDGET);
        }
        s.pick(9999, false, false);
        let event = s.event("selection");
        assert_eq!(event["focus_id"], "/fixture/file-9999");
        assert_eq!(event["selected_ids"], json!(["/fixture/file-9999"]));
        view["revision"] = json!(2);
        assert!(s.ingest(&view));
        assert_eq!(s.model_loads, 1);
        view["total"] = json!(9999);
        view["revision"] = json!(3);
        assert!(!s.ingest(&view));
        assert_eq!(s.total, 10000);
        let loading = json!({"generation":2,"revision":1,"total":0,"offset":0,"rows":[]});
        assert!(s.ingest(&loading));
        assert!(s.dataset.is_none());
        assert!(s.rows.is_empty());
    }
    #[test]
    fn failed_source_does_not_retry_io_each_frame() {
        let mut s = State::default();
        s.session = "s".into();
        s.model_owner = "table".into();
        let view = json!({"generation":1,"revision":1,"total":1,"offset":0,"rows":[],"source":{"path":"/nonexistent/model","token":"t"}});
        assert!(!s.ingest(&view));
        assert!(!s.ingest(&view));
        assert_eq!(s.model_loads, 1);
        assert_eq!(s.refused, 1);
        assert!(s.model_error.is_some());
    }
    fn page(g: u64, r: u64, offset: usize) -> Value {
        json!({"generation":g,"revision":r,"offset":offset,"total":10000,
            "rows":(offset..offset+128).map(|i| json!({"id":format!("row-{i}"),"cells":[format!("row-{i}")]})).collect::<Vec<_>>()})
    }
    #[test]
    fn selection_ranges_toggle_split_merge_and_select_all_without_row_materialisation() {
        let mut s = Selection::default();
        s.range(0, 1_000_000);
        s.toggle(400);
        assert_eq!(s.0, [(0, 400), (401, 1_000_000)]);
        s.toggle(400);
        assert_eq!(s.0, [(0, 1_000_000)]);
        assert_eq!(s.count(), 1_000_000);
    }
    #[test]
    fn stale_transactions_cannot_replace_generation_or_row_identity() {
        let mut s = State::default();
        assert!(s.ingest(&page(2, 1, 0)));
        assert!(!s.ingest(&page(1, 20, 0)));
        assert_eq!(s.generation, 2);
        let mut bad = page(2, 2, 0);
        bad["rows"][0]["id"] = json!("different");
        assert!(!s.ingest(&bad));
        assert_eq!(s.revision, 1);
    }
    #[test]
    fn native_selection_survives_page_updates_and_cache_is_bounded() {
        let mut s = State::default();
        s.ingest(&page(1, 1, 0));
        s.pick(5, false, false);
        for n in 1..50 {
            s.request = Some(n * 128);
            s.ingest(&page(1, n as u64 + 1, n * 128));
        }
        assert!(s.rows.len() <= ROW_BUDGET);
        assert!(s.selection.contains(5));
        assert_eq!(s.focus, Some(5));
        s.ingest(&page(2, 1, 0));
        assert_eq!(s.selection.count(), 0);
        assert_eq!(s.scroll_to, Some(0));
    }
}
