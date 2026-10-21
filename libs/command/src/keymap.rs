// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Key binding data and conflict policy, independent of a windowing toolkit.
use std::collections::{BTreeMap, BTreeSet};

pub type Overrides = BTreeMap<String, Vec<String>>;
type Bound = (Option<String>, Vec<String>);

/// Store modifier prefixes in Ctrl, Alt, Shift order, preserving the key grammar.
fn canonical_chord(text: &str) -> String {
    let parts: Vec<_> = text.split('+').collect();
    let (key, modifiers) = parts.split_last().expect("split always has a part");
    if modifiers
        .iter()
        .any(|m| !["Ctrl", "Alt", "Shift"].contains(m))
    {
        return text.to_owned();
    }
    let mut out = String::new();
    for modifier in ["Ctrl", "Alt", "Shift"] {
        if modifiers.contains(&modifier) {
            out.push_str(modifier);
            out.push('+');
        }
    }
    out.push_str(key);
    out
}

fn is_user_fkey(chord: &str) -> bool {
    let key = chord.rsplit('+').next().unwrap_or(chord).to_ascii_lowercase();
    matches!(
        key.as_str(),
        "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12"
    )
}

fn ensure_not_user_fkey(chord: &str) {
    assert!(!is_user_fkey(chord), "F1-F12 belong to the user");
}

#[derive(Default)]
pub(crate) struct Bindings {
    current: BTreeMap<&'static str, Bound>,
    defaults: Option<BTreeMap<&'static str, Bound>>,
    overridden: BTreeSet<&'static str>,
}

impl Bindings {
    pub fn add(&mut self, id: &'static str, own: Option<String>) {
        let own = own.map(|s| canonical_chord(&s));
        if let Some(s) = &own {
            ensure_not_user_fkey(s);
        }
        assert!(
            !self.current.contains_key(id),
            "duplicate command id {id:?}"
        );
        if let Some(s) = &own {
            assert!(
                !self.taken(s),
                "duplicate shortcut {s:?} on {id:?} (chord text decides duplicates)"
            );
        }
        let bound = (own, Vec::new());
        if let Some(defaults) = &mut self.defaults {
            defaults.insert(id, bound.clone());
        }
        self.current.insert(id, bound);
    }

    fn taken(&self, shortcut: &str) -> bool {
        self.current.values().any(|(own, more)| {
            own.as_deref() == Some(shortcut) || more.iter().any(|s| s == shortcut)
        })
    }

    pub fn set_shortcut(&mut self, id: &'static str, own: Option<String>) {
        let own = own.map(|s| canonical_chord(&s));
        if let Some(s) = &own {
            ensure_not_user_fkey(s);
        }
        if let Some(s) = &own {
            assert!(
                !self.taken(s),
                "duplicate shortcut {s:?} on {id:?} (chord text decides duplicates)"
            );
        }
        self.current.get_mut(id).expect("known command").0 = own;
    }

    pub fn add_shortcuts(&mut self, id: &'static str, shortcuts: impl IntoIterator<Item = String>) {
        for s in shortcuts {
            let s = canonical_chord(&s);
            ensure_not_user_fkey(&s);
            if !self.taken(&s) {
                self.current.get_mut(id).expect("known command").1.push(s);
            }
        }
    }

    pub fn shortcuts_of(&self, id: &str) -> Vec<String> {
        self.current
            .get(id)
            .map(|(own, more)| own.iter().chain(more).cloned().collect())
            .unwrap_or_default()
    }

    pub fn bound_of(&self, id: &str) -> Bound {
        self.current.get(id).cloned().unwrap_or_default()
    }

    pub fn is_overridden(&self, id: &str) -> bool {
        self.overridden.contains(id)
    }

    pub fn rebind(
        &mut self,
        overrides: &Overrides,
        parse: impl Fn(&str) -> Option<String>,
    ) -> Vec<String> {
        let defaults = self
            .defaults
            .get_or_insert_with(|| self.current.clone())
            .clone();
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
                    Some(s) if parsed.contains(&s) => {
                        notes.push(format!("{action}: {text:?} is listed twice"))
                    }
                    Some(s) => parsed.push(s),
                }
            }
            if !chords.is_empty() && parsed.is_empty() {
                notes.push(format!(
                    "{action}: no key it names is known; the defaults stay"
                ));
                continue;
            }
            let mut list = Vec::new();
            for s in parsed {
                if claimed.insert(s.clone()) {
                    list.push(s);
                } else {
                    notes.push(format!("{action}: {s:?} is bound to another command"));
                }
            }
            let mut list = list.into_iter();
            chosen.insert(id, (list.next(), list.collect()));
        }
        let free = |s: &String| !claimed.contains(s);
        let mut bound: BTreeMap<_, _> = defaults
            .into_iter()
            .filter(|(id, _)| !chosen.contains_key(id))
            .map(|(id, (own, mut more))| {
                more.retain(free);
                let own = match own {
                    Some(s) if free(&s) => Some(s),
                    Some(_) if !more.is_empty() => Some(more.remove(0)),
                    _ => None,
                };
                (id, (own, more))
            })
            .collect();
        self.overridden = chosen.keys().copied().collect();
        bound.extend(chosen);
        self.current = bound;
        notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "F1-F12 belong to the user")]
    fn bare_f1_default_is_rejected_case_insensitively() {
        let mut bindings = Bindings::default();
        bindings.add("cmd.bare_fkey", Some("f1".into()));
    }

    #[test]
    #[should_panic(expected = "F1-F12 belong to the user")]
    fn modified_fkey_default_is_rejected() {
        let mut bindings = Bindings::default();
        bindings.add("cmd.modified_fkey", Some("Ctrl+Alt+F12".into()));
    }

    #[test]
    #[should_panic(expected = "F1-F12 belong to the user")]
    fn extra_fkey_shortcut_is_rejected() {
        let mut bindings = Bindings::default();
        bindings.add("cmd.extra_fkey", None);
        bindings.add_shortcuts("cmd.extra_fkey", ["Shift+F10".to_string()]);
    }

    #[test]
    #[should_panic(expected = "F1-F12 belong to the user")]
    fn set_shortcut_fkey_is_rejected() {
        let mut bindings = Bindings::default();
        bindings.add("cmd.set_fkey", None);
        bindings.set_shortcut("cmd.set_fkey", Some("Alt+F2".into()));
    }

    #[test]
    fn f13_remains_valid_as_default() {
        let mut bindings = Bindings::default();
        bindings.add("cmd.f13", Some("Ctrl+F13".into()));
        assert_eq!(bindings.shortcuts_of("cmd.f13"), vec!["Ctrl+F13".to_string()]);
    }

    #[test]
    fn rebind_accepts_user_fkey_override() {
        let mut bindings = Bindings::default();
        bindings.add("cmd.user_fkey", Some("Ctrl+O".into()));

        let overrides = Overrides::from([(
            "cmd.user_fkey".to_string(),
            vec!["Ctrl+F1".to_string()],
        )]);

        let notes = bindings.rebind(&overrides, |text| Some(text.to_string()));

        assert!(notes.is_empty());
        assert_eq!(
            bindings.shortcuts_of("cmd.user_fkey"),
            vec!["Ctrl+F1".to_string()]
        );
    }

    #[test]
    #[should_panic(expected = "F1-F12 belong to the user")]
    fn extra_fkey_shortcut_is_rejected_before_duplicate_filtering() {
        let mut bindings = Bindings::default();
        bindings.add("cmd.user_fkey", Some("Ctrl+O".into()));
        bindings.add("cmd.extra_fkey", None);

        let overrides = Overrides::from([(
            "cmd.user_fkey".to_string(),
            vec!["F1".to_string()],
        )]);
        let _ = bindings.rebind(&overrides, |text| Some(text.to_string()));

        bindings.add_shortcuts("cmd.extra_fkey", ["F1".to_string()]);
    }
}
