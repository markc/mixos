// SPDX-License-Identifier: MIT OR Apache-2.0
//! Discovery-only stable-ID model. Point edits do not walk or copy the model.
use super::model::{Row, State, TableInteger};
use serde_json::Value;
use std::{collections::{HashMap, HashSet}, sync::Arc};

const MAX_ROWS: usize = 1_000_000;
const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_DELTA: usize = 256;

#[derive(Debug, Default)]
pub(super) struct Model {
    order: Vec<Arc<str>>,
    rows: HashMap<Arc<str>, Arc<Row>>,
    bytes: usize,
}

fn row(raw: &Value) -> Result<Arc<Row>, String> {
    let id = raw["id"].as_str().ok_or("missing id")?;
    let cells = raw["cells"].as_array().ok_or("missing cells")?;
    if id.is_empty() || id.len() > 4096 || cells.is_empty() || cells.len() > 16
        || cells.iter().any(|s| s.as_str().is_none_or(|s| s.len() > 4096)) {
        return Err("invalid row".into());
    }
    let edit = raw["edit"].as_str().unwrap_or("");
    if edit.len() > 4096 { return Err("edit too long".into()); }
    Ok(Arc::new(Row { id: id.into(), cells: cells.iter().map(|v| v.as_str().unwrap().into()).collect(), edit: edit.into(), raw: raw.clone() }))
}

fn bytes(r: &Row) -> usize {
    // Byte accounting is retained logical string/JSON bytes, not allocator RSS.
    r.id.len() + r.edit.len() + r.cells.iter().map(String::len).sum::<usize>() + r.raw.to_string().len()
}

impl Model {
    pub fn len(&self) -> usize { self.order.len() }
    pub fn id(&self, i: usize) -> Option<&str> { self.order.get(i).map(AsRef::as_ref) }
    pub fn row(&self, i: usize) -> Option<Arc<Row>> { self.rows.get(self.id(i)?).cloned() }

    /// Validate and allocate changed data first; commit only after every check.
    fn apply(&mut self, delta: &Value) -> Result<Vec<usize>, String> {
        let kind = delta["kind"].as_str().ok_or("missing delta kind")?;
        match kind {
            "edit" => {
                let edits = delta["edits"].as_array().ok_or("missing edits")?;
                if edits.len() > MAX_DELTA { return Err("delta row budget".into()); }
                let mut next = Vec::with_capacity(edits.len());
                let mut seen = HashSet::new();
                let mut size = self.bytes;
                for edit in edits {
                    let i = edit["index"].whole().ok_or("bad index")? as usize;
                    let r = row(&edit["row"])?;
                    if self.id(i) != Some(r.id.as_str()) || !seen.insert(i) { return Err("stale/duplicate identity".into()); }
                    size = size - bytes(self.rows.get(r.id.as_str()).unwrap()) + bytes(&r);
                    next.push((i, r));
                }
                if size > MAX_BYTES { return Err("model byte budget".into()); }
                let indexes = next.iter().map(|(i, _)| *i).collect();
                for (_, r) in next { self.rows.insert(Arc::from(r.id.as_str()), r); }
                self.bytes = size;
                Ok(indexes)
            }
            "splice" => {
                let index = delta["index"].whole().ok_or("bad splice index")? as usize;
                let deleted = delta["delete"].as_array().ok_or("missing deletion identities")?;
                let inserted = delta["insert"].as_array().ok_or("missing inserted rows")?;
                if deleted.len() + inserted.len() > MAX_DELTA || index > self.len()
                    || deleted.len() > self.len() - index || self.len() - deleted.len() + inserted.len() > MAX_ROWS {
                    return Err("splice bounds/budget".into());
                }
                for (j, id) in deleted.iter().enumerate() {
                    if id.as_str() != self.id(index+j) { return Err("splice deletion identity".into()); }
                }
                let removed: HashSet<&str> = deleted.iter().map(|v| v.as_str().unwrap()).collect();
                let mut seen = HashSet::new();
                let mut next = Vec::with_capacity(inserted.len());
                let mut size = self.bytes;
                for id in &removed { size -= bytes(self.rows.get(*id).unwrap()); }
                for raw in inserted {
                    let r = row(raw)?;
                    if !seen.insert(r.id.clone()) || (self.rows.contains_key(r.id.as_str()) && !removed.contains(r.id.as_str())) {
                        return Err("duplicate inserted identity".into());
                    }
                    size += bytes(&r); next.push(r);
                }
                if size > MAX_BYTES { return Err("model byte budget".into()); }
                for id in removed { self.rows.remove(id); }
                let ids: Vec<Arc<str>> = next.iter().map(|r| Arc::from(r.id.as_str())).collect();
                for (id, r) in ids.iter().zip(next) { self.rows.insert(Arc::clone(id), r); }
                self.order.splice(index..index+deleted.len(), ids);
                self.bytes = size;
                Ok(Vec::new())
            }
            "reverse" => { self.order.reverse(); Ok(Vec::new()) }
            "sort" => {
                let column=delta["column"].whole().filter(|n|*n<16).ok_or("sort column")? as usize;
                let ascending=delta["ascending"].as_bool().ok_or("sort direction")?;
                let mut keys:Vec<_>=self.order.iter().map(|id|(Arc::clone(id),self.rows[id].cells.get(column).map(String::as_str).unwrap_or(""))).collect();
                keys.sort_by(|a,b| if ascending {a.1.cmp(b.1)}else{b.1.cmp(a.1)});
                self.order=keys.into_iter().map(|(id,_)|id).collect();
                Ok(Vec::new())
            }
            "order" => {
                let ids = delta["ids"].as_array().ok_or("missing permutation")?;
                if ids.len() != self.len() { return Err("permutation count".into()); }
                let mut seen = HashSet::with_capacity(ids.len());
                let mut next = Vec::with_capacity(ids.len());
                for id in ids {
                    let id = id.as_str().ok_or("bad permutation id")?;
                    if !seen.insert(id) { return Err("duplicate permutation id".into()); }
                    let (key, _) = self.rows.get_key_value(id).ok_or("unknown permutation id")?;
                    next.push(Arc::clone(key));
                }
                self.order = next;
                Ok(Vec::new())
            }
            _ => Err("unknown delta kind".into()),
        }
    }
}

impl State {
    pub(super) fn ingest_delta(&mut self, view: &Value) -> bool {
        if view["generation"].whole() == Some(self.generation) && view["revision"].whole() == Some(self.revision) { return false; }
        let result = self.try_delta(view);
        if result.is_err() { self.refused += 1; }
        result.is_ok()
    }

    fn try_delta(&mut self, view: &Value) -> Result<(), String> {
        let generation = view["generation"].whole().ok_or("generation")?;
        let revision = view["revision"].whole().ok_or("revision")?;
        if generation == self.generation && revision == self.revision { return Err("already applied".into()); }
        if generation == 0 || view.get("source").is_some() { return Err("generation/source".into()); }
        let reset = generation > self.generation && view["delta"]["reset"].as_bool() == Some(true);
        if (!reset && generation != self.generation)
            || view["delta"]["base_revision"].whole() != Some(if reset {0} else {self.revision})
            || revision != if reset {1} else {self.revision + 1} {
            return Err("stale/gapped delta".into());
        }
        let structural = view["delta"]["kind"] != "edit";
        let mut selected = HashSet::new();
        let mut focus = None;
        let mut anchor = None;
        if structural && !reset {
            if let Some(model) = &self.incremental {
                for &(a,b) in &self.selection.0 {
                    for i in a..b { if let Some(id) = model.id(i) { selected.insert(id.to_owned()); } }
                }
                focus = self.focus.and_then(|i|model.id(i)).map(str::to_owned);
                anchor = self.anchor.and_then(|i|model.id(i)).map(str::to_owned);
            }
        }
        let touched = if reset {
            let mut model = Model::default();
            let touched = model.apply(&view["delta"])?;
            self.incremental = Some(model); touched
        } else {
            self.incremental.as_mut().ok_or("no model")?.apply(&view["delta"])?
        };
        if reset {
            self.rows.clear(); self.dataset = None;
            self.selection = Default::default(); self.focus = None; self.anchor = None;
            self.edit = None; self.request = None; self.scroll_to = Some(0);
        } else if structural {
            self.rows.clear(); self.selection = Default::default(); self.focus = None; self.anchor = None;
            // Structural operations preserve identities. O(N) order/selection repair
            // is explicit and benchmarked separately from O(k) point edits.
            let model = self.incremental.as_ref().unwrap();
            if !selected.is_empty() || focus.is_some() || anchor.is_some() {
            for (i, id) in model.order.iter().enumerate() {
                if selected.contains(id.as_ref()) {
                    if let Some(last) = self.selection.0.last_mut().filter(|(_,b)| *b==i) { last.1=i+1; }
                    else { self.selection.0.push((i,i+1)); }
                }
                if focus.as_deref()==Some(id.as_ref()) { self.focus=Some(i); }
                if anchor.as_deref()==Some(id.as_ref()) { self.anchor=Some(i); }
            }
            }
            if let Some(edit) = &mut self.edit {
                if let Some(i) = model.order.iter().position(|id| id.as_ref()==edit.id) { edit.index=i; }
                else { self.edit=None; }
            }
        } else {
            for i in touched { self.rows.remove(&i); }
        }
        self.total = self.incremental.as_ref().unwrap().len();
        self.generation = generation; self.revision = revision; self.updates += 1;
        Ok(())
    }
}
