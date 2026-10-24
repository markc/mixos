// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded, transactional document snapshots. The current and saved snapshots
//! are retained independently of the undo/redo budget.
use std::{collections::VecDeque, sync::Arc};

#[derive(Debug, Clone, Copy)]
pub struct Budget<D> {
    pub max_entries: usize,
    pub max_bytes: usize,
    pub bytes: fn(&D) -> usize,
}

#[derive(Debug, Clone)]
struct Entry<D> {
    document: Arc<D>,
    label: &'static str,
}

#[derive(Debug, Clone)]
pub struct History<D> {
    current: Arc<D>,
    saved: Arc<D>,
    undo: VecDeque<Entry<D>>,
    redo: Vec<Entry<D>>,
    budget: Budget<D>,
    coalesce: Option<&'static str>,
}

impl<D: Clone + PartialEq> History<D> {
    pub fn new(initial: D, budget: Budget<D>) -> Self {
        let current = Arc::new(initial);
        Self {
            saved: current.clone(),
            current,
            undo: VecDeque::new(),
            redo: Vec::new(),
            budget,
            coalesce: None,
        }
    }

    pub fn current(&self) -> &D {
        &self.current
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
    /// Undo labels, oldest first, and redo labels, next redo first.
    pub fn labels(&self) -> (Vec<&'static str>, Vec<&'static str>) {
        (
            self.undo.iter().map(|e| e.label).collect(),
            self.redo.iter().rev().map(|e| e.label).collect(),
        )
    }
    pub fn mark_saved(&mut self) {
        self.saved = self.current.clone();
        self.coalesce = None;
    }
    pub fn dirty(&self) -> bool {
        self.current != self.saved
    }

    pub fn edit<R, E>(
        &mut self,
        label: &'static str,
        f: impl FnOnce(&mut D) -> Result<R, E>,
    ) -> Result<R, E> {
        self.apply(label, false, f)
    }

    /// Consecutive successful calls with the same key share one undo step.
    /// An ordinary edit, undo, redo or saved checkpoint ends the group.
    pub fn edit_coalesced<R, E>(
        &mut self,
        key: &'static str,
        f: impl FnOnce(&mut D) -> Result<R, E>,
    ) -> Result<R, E> {
        self.apply(key, true, f)
    }

    fn apply<R, E>(
        &mut self,
        label: &'static str,
        coalesce: bool,
        f: impl FnOnce(&mut D) -> Result<R, E>,
    ) -> Result<R, E> {
        let mut next = (*self.current).clone();
        let result = f(&mut next)?;
        if !coalesce || self.coalesce != Some(label) || self.undo.is_empty() {
            self.undo.push_back(Entry {
                document: self.current.clone(),
                label,
            });
        }
        self.current = Arc::new(next);
        self.redo.clear();
        self.coalesce = coalesce.then_some(label);
        self.trim();
        Ok(result)
    }

    fn trim(&mut self) {
        while self.undo.len() + self.redo.len() > self.budget.max_entries
            || self
                .undo
                .iter()
                .chain(self.redo.iter())
                .map(|e| (self.budget.bytes)(&e.document))
                .sum::<usize>()
                > self.budget.max_bytes
        {
            if self.undo.pop_front().is_none() {
                self.redo.remove(0);
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        self.coalesce = None;
        let Some(entry) = self.undo.pop_back() else {
            return false;
        };
        let previous = std::mem::replace(&mut self.current, entry.document);
        self.redo.push(Entry {
            document: previous,
            label: entry.label,
        });
        self.trim();
        true
    }
    pub fn redo(&mut self) -> bool {
        self.coalesce = None;
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        let previous = std::mem::replace(&mut self.current, entry.document);
        self.undo.push_back(Entry {
            document: previous,
            label: entry.label,
        });
        self.trim();
        true
    }
}
