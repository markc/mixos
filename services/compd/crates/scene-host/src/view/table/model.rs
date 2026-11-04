// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lab compound-view state. Logical indexes are valid only within a generation.
use std::collections::BTreeMap;
use std::sync::Arc;

use egui::{Color32, FontId, Galley, Ui};
use serde_json::{Value, json};

pub const ROW_BUDGET: usize = 768;
pub const TEXT_BUDGET: usize = 1024;
pub const PAGE: usize = 128;

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

pub struct Edit {
    pub index: usize,
    pub id: String,
    pub text: String,
    pub pending: Option<u64>,
    pub focus: bool,
}

struct Text {
    text: String,
    font: FontId,
    epoch: u64,
    used: u64,
    galley: Arc<Galley>,
}

#[derive(Default)]
pub struct State {
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
    pub old_active: bool,
    pub request: Option<usize>,
    pub stale: u64,
    pub refused: u64,
    pub updates: u64,
    pub shapes: u64,
    pub hits: u64,
    pub text_epoch: u64,
    texts: BTreeMap<(usize, usize), Text>,
    clock: u64,
}

impl State {
    /// Validate the whole page before changing state. A generation is a row-order
    /// identity; changing a row's identity in place requires a new generation.
    pub fn ingest(&mut self, view: &Value) -> bool {
        let (Some(generation), Some(revision), Some(total), Some(offset), Some(rows)) = (
            view["generation"].as_u64(),
            view["revision"].as_u64(),
            view["total"].as_u64(),
            view["offset"].as_u64(),
            view["rows"].as_array(),
        ) else {
            self.refused += 1;
            return false;
        };
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
        let epoch = view["selection_epoch"].as_u64().unwrap_or(0);
        let reset = generation != self.generation;
        let mut incoming_selection=Selection::default();
        if let Some(selection)=view.get("selection") {
            let Some(ranges)=selection["ranges"].as_array() else {self.refused+=1; return false;};
            let mut end=0;
            for range in ranges {
                let (Some(a),Some(b))=(range[0].as_u64(),range[1].as_u64()) else {self.refused+=1; return false;};
                if a>=b || b>total || a<end {self.refused+=1; return false;}
                incoming_selection.0.push((a as usize,b as usize)); end=b;
            }
        }
        if reset {
            self.rows.clear();
            self.texts.clear();
            self.selection = Selection::default();
            self.focus = None;
            self.anchor = None;
            self.edit = None;
            self.request = None;
            self.selection_epoch = 0;
            self.first = 0;
            self.end = 0;
            self.scroll_y = 0.0;
            self.scroll_to = Some(0);
        }
        if reset || epoch > self.selection_epoch {
            self.selection_epoch = epoch;
            self.focus = view["focus"]
                .as_u64()
                .map(|v| v as usize)
                .filter(|&v| v < total as usize);
            self.selection = incoming_selection;
            self.anchor = self.focus;
            self.scroll_to = self.focus;
        }
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
            self.texts.retain(|&(i, _), _| i != index);
        }
        if self
            .edit
            .as_ref()
            .and_then(|e| e.pending)
            .is_some_and(|seq| view["ack"].as_u64().unwrap_or(0) >= seq)
        {
            self.edit = None;
        }
        true
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
        json!({"kind":kind,"session":self.session,"generation":self.generation,"seq":self.seq,
            "focus":self.focus,"selection":{"ranges":self.selection.0}})
    }

    pub fn galley(
        &mut self,
        ui: &Ui,
        index: usize,
        col: usize,
        text: &str,
        font: FontId,
    ) -> Arc<Galley> {
        self.clock += 1;
        if let Some(cached) = self.texts.get_mut(&(index, col))
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
            (index, col),
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
        json!({"session":self.session,"generation":self.generation,"revision":self.revision,"seq":self.seq,
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
    }
}
