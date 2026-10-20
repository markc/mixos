// SPDX-License-Identifier: MIT OR Apache-2.0
//! Key binding data and conflict policy, independent of a windowing toolkit.
use std::collections::{BTreeMap, BTreeSet};

pub type Overrides = BTreeMap<String, Vec<String>>;
type Bound = (Option<String>, Vec<String>);

#[derive(Default)]
pub(crate) struct Bindings {
    built: BTreeMap<&'static str, Bound>,
    current: BTreeMap<&'static str, Bound>,
    defaults: Option<BTreeMap<&'static str, Bound>>,
}

impl Bindings {
    pub fn add(&mut self, id: &'static str, own: Option<String>) {
        assert!(!self.current.contains_key(id), "duplicate command id {id:?}");
        if let Some(s) = &own { assert!(!self.taken(s), "duplicate shortcut on {id:?}"); }
        self.current.insert(id, (own.clone(), Vec::new()));
        self.built.insert(id, (own, Vec::new()));
    }

    fn taken(&self, shortcut: &str) -> bool {
        self.current.values().any(|(own, more)| own.as_deref() == Some(shortcut) || more.iter().any(|s| s == shortcut))
    }

    pub fn set_shortcut(&mut self, id: &'static str, own: Option<String>) {
        if let Some(s) = &own { assert!(!self.taken(s), "duplicate shortcut on {id:?}"); }
        self.current.get_mut(id).expect("known command").0 = own.clone();
        self.built.get_mut(id).expect("known command").0 = own;
    }

    pub fn add_shortcuts(&mut self, id: &'static str, shortcuts: impl IntoIterator<Item = String>) {
        for s in shortcuts {
            if !self.taken(&s) {
                self.current.get_mut(id).expect("known command").1.push(s.clone());
                self.built.get_mut(id).expect("known command").1.push(s);
            }
        }
    }

    pub fn shortcuts_of(&self, id: &str) -> Vec<String> {
        self.current.get(id).map(|(own, more)| own.iter().chain(more).cloned().collect()).unwrap_or_default()
    }

    pub fn rebind(&mut self, overrides: &Overrides, parse: impl Fn(&str) -> Option<String>) -> Vec<String> {
        let defaults = self.defaults.get_or_insert_with(|| self.current.clone()).clone();
        let mut notes = Vec::new();
        let mut chosen = BTreeMap::new();
        let mut claimed = BTreeSet::new();
        for (action, chords) in overrides {
            let Some((&id, _)) = self.current.get_key_value(action.as_str()) else {
                notes.push(format!("{action}: no such command"));
                continue;
            };
            let mut parsed = Vec::new();
            for text in chords {
                match parse(text) {
                    None => notes.push(format!("{action}: {text:?} is not a key")),
                    Some(s) if parsed.contains(&s) => notes.push(format!("{action}: {text:?} is listed twice")),
                    Some(s) => parsed.push(s),
                }
            }
            if !chords.is_empty() && parsed.is_empty() {
                notes.push(format!("{action}: no key it names is known; the defaults stay"));
                continue;
            }
            let mut list = Vec::new();
            for s in parsed {
                if claimed.insert(s.clone()) { list.push(s); }
                else { notes.push(format!("{action}: {s:?} is bound to another command")); }
            }
            let mut list = list.into_iter();
            chosen.insert(id, (list.next(), list.collect()));
        }
        let free = |s: &String| !claimed.contains(s);
        let mut bound: BTreeMap<_, _> = defaults.into_iter()
            .filter(|(id, _)| !chosen.contains_key(id))
            .map(|(id, (own, mut more))| {
                more.retain(free);
                let own = match own {
                    Some(s) if free(&s) => Some(s),
                    Some(_) if !more.is_empty() => Some(more.remove(0)),
                    _ => None,
                };
                (id, (own, more))
            }).collect();
        bound.extend(chosen);
        self.current = bound;
        notes
    }
}
