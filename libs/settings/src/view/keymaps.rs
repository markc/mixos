// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `keymaps.*`: the person's bindings per app and action, and the
//! compositor's spawn entries.
use super::{Reader, View};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keymaps {
    /// Per app, per action, the chords chosen (today's [`crate::keymap`]
    /// shape); an empty list unbinds. An action not listed keeps the app's
    /// own binding.
    pub apps: crate::keymap::Keymaps,
    /// `keymaps.comp.spawn.<name>`: commands compd runs on a chord.
    pub spawn: BTreeMap<String, Spawn>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spawn {
    pub command: Vec<String>,
    pub bindings: Vec<String>,
}

impl View for Keymaps {
    fn read(r: &Reader) -> Self {
        let mut view = Self::default();
        let paths: Vec<&String> = r
            .values()
            .keys()
            .filter(|k| k.starts_with("keymaps."))
            .collect();
        for path in paths {
            let Some(value) = r.authored(path) else {
                continue;
            };
            if let Some(name) = path.strip_prefix("keymaps.comp.spawn.") {
                view.spawn.insert(
                    name.to_owned(),
                    Spawn {
                        command: super::strings(&value["command"]),
                        bindings: super::strings(&value["bindings"]),
                    },
                );
            } else if let Some((app, action)) = path["keymaps.".len()..].split_once('.') {
                view.apps
                    .entry(app.to_owned())
                    .or_default()
                    .insert(action.to_owned(), super::strings(&value));
            }
        }
        view
    }
}

impl Keymaps {
    /// `app`'s changes, empty when it has none.
    pub fn app(&self, app: &str) -> BTreeMap<String, Vec<String>> {
        self.apps.get(app).cloned().unwrap_or_default()
    }
}
