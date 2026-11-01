// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Content that comes from data, not from page data: the networks, devices,
//! outputs, users and streams a page lists. A section names a live source
//! ([`pages::Section::items`]) and a choice can take extra options from one
//! ([`pages::Row::options_from`]); the owner of the data (hostd, monitord,
//! the audio service, … as their backends land) fills the source with
//! [`Engine::set_items`] / [`Engine::set_options`]. Items stay data to the
//! rest of Prefs: `prefs.page` lists them, search finds them, and their
//! actions (connect, forget, remove) go through one command, `item`, by the
//! same path as a click.
//!
//! This module is also where a Bus write operates a control: `prefs.set`
//! finds the row bound to the key and applies its enable rules before the
//! write, so an agent can do exactly what the window lets a person do.
use crate::engine::{Effect, Engine};
use crate::pages::{self, Hit};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// One row from data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Item {
    /// Stable within its source (an SSID, a connector, a device address).
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Trailing text (Connected, 72%).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub actions: Vec<ItemAction>,
}

/// A button on an item row; its owner carries it out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ItemAction {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}

/// The live lists and options, by source.
#[derive(Clone, Debug, Default)]
pub struct Live {
    items: BTreeMap<String, Vec<Item>>,
    options: BTreeMap<String, Vec<(Value, String)>>,
}

impl Engine {
    /// The owner of `source` replaces its list.
    pub fn set_items(&mut self, source: &str, items: Vec<Item>) {
        self.live.items.insert(source.to_owned(), items);
        self.refresh_hits();
    }

    /// The owner of `source` replaces its options (value, shown label).
    pub fn set_options(&mut self, source: &str, options: Vec<(Value, String)>) {
        self.live.options.insert(source.to_owned(), options);
    }

    pub fn items(&self, source: &str) -> &[Item] {
        self.live.items.get(source).map_or(&[], Vec::as_slice)
    }

    pub fn live_options(&self, source: &str) -> &[(Value, String)] {
        self.live.options.get(source).map_or(&[], Vec::as_slice)
    }

    /// Every item `page` lists.
    pub fn items_of_page(&self, page: &str) -> impl Iterator<Item = &Item> {
        pages::get(page)
            .into_iter()
            .flat_map(|p| p.sections().iter())
            .filter_map(|s| s.items)
            .flat_map(|i| self.items(i.source))
    }

    /// Items whose title or description contains `query`.
    pub(crate) fn find_items(&self, query: &str) -> Vec<Hit> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        for page in pages::all() {
            for section in page.sections() {
                let Some(items) = section.items else { continue };
                for item in self.items(items.source) {
                    let text = format!(
                        "{}\n{}",
                        item.title,
                        item.description.as_deref().unwrap_or_default()
                    )
                    .to_lowercase();
                    if text.contains(&query) {
                        hits.push(Hit {
                            page: page.id,
                            section: Some(section.id),
                            row: None,
                            item: Some(item.id.clone()),
                        });
                    }
                }
            }
        }
        hits
    }

    /// Run `action` on item `id` of `source`, as its button does: refused
    /// when the item, the action or its page's backend is missing.
    pub fn item_action(&mut self, source: &str, id: &str, action: &str) -> Result<(), String> {
        let page = pages::all()
            .iter()
            .find(|p| p.sections().iter().any(|s| s.items.is_some_and(|i| i.source == source)))
            .ok_or_else(|| self.label("item-unknown", &[("item", source)]))?;
        if let Some(reason) = self.page_reason(page) {
            return Err(reason);
        }
        let known = self
            .items(source)
            .iter()
            .find(|i| i.id == id)
            .and_then(|i| i.actions.iter().find(|a| a.id == action))
            .is_some_and(|a| a.enabled);
        if !known {
            return Err(self.label("item-unknown", &[("item", id)]));
        }
        self.effects.push(Effect::Live {
            source: source.to_owned(),
            item: id.to_owned(),
            action: action.to_owned(),
        });
        Ok(())
    }

    /// A choice row's options as drawn: the catalogue's variants when it
    /// describes the key as an enum (the page's options give their labels
    /// and order; a variant the page lacks is added, one settingsd dropped
    /// is left out), then any live options. The page data alone is only
    /// the fallback before `settings.describe` answers.
    pub fn choices(&self, row: &pages::Row) -> Vec<(Value, String)> {
        let pages::Control::Choice { key, options, .. } = &row.control else {
            return Vec::new();
        };
        let variants = match self.store.catalog.get(key).map(|i| &i.kind) {
            Some(crate::catalog::Kind::Enum(names)) => Some(names),
            _ => None,
        };
        let mut out: Vec<(Value, String)> = options
            .iter()
            .filter(|o| match (variants, o.value) {
                (Some(names), pages::Lit::S(s)) => names.iter().any(|n| n == s),
                _ => true,
            })
            .map(|o| (o.value.value(), self.label(o.label, &[])))
            .collect();
        for name in variants.into_iter().flatten() {
            if !out.iter().any(|(v, _)| v.as_str() == Some(name)) {
                let key = format!("opt-{}", name.replace('_', "-"));
                let text = self.label(&key, &[]);
                out.push((Value::from(name.as_str()), if text == key { name.clone() } else { text }));
            }
        }
        if let Some(source) = row.options_from {
            for (value, text) in self.live_options(source) {
                if !out.iter().any(|(v, _)| v == value) {
                    out.push((value.clone(), text.clone()));
                }
            }
        }
        out
    }

    /// A numeric control's range: the catalogue's, else the page's.
    pub fn bounds(&self, key: &str, min: f64, max: f64) -> (f64, f64) {
        match self.store.catalog.get(key).map(|i| &i.kind) {
            Some(crate::catalog::Kind::Int { min, max, .. }) => (*min as f64, *max as f64),
            Some(crate::catalog::Kind::Number { min, max, .. })
                if min.is_finite() && max.is_finite() =>
            {
                (*min, *max)
            }
            _ => (min, max),
        }
    }

    /// A steps control's values: the catalogue's set, else the page's.
    pub fn steps(&self, key: &str, steps: &[pages::Lit]) -> Vec<Value> {
        match self.store.catalog.get(key).map(|i| &i.kind) {
            Some(crate::catalog::Kind::Set(values)) if !values.is_empty() => {
                values.iter().map(|v| Value::from(*v)).collect()
            }
            _ => steps.iter().map(|s| s.value()).collect(),
        }
    }

    /// The control bound to `key`: its page and row, the first in page
    /// order.
    pub fn control_of(&self, key: &str) -> Option<(&'static pages::Page, &'static pages::Row)> {
        pages::all().iter().find_map(|page| {
            page.rows()
                .map(|(_, row)| row)
                .find(|row| row.control.key() == Some(key))
                .map(|row| (page, row))
        })
    }

    /// `prefs.set {key, value}`: operate the control bound to `key` as a
    /// person would. The control's enable rules (a missing backend, a
    /// pinned design source, a stale mirror) refuse it as they disable it,
    /// and the write is the control's own fenced `settings.apply`. A key no
    /// page shows yet has no control here: settingsd takes it directly.
    pub fn set_control(&mut self, key: &str, value: Value) -> Result<(), String> {
        let (page, row) = self
            .control_of(key)
            .ok_or_else(|| self.label("set-no-control", &[("key", key)]))?;
        if let Some(why) = self.row_blocked(page, row) {
            return Err(why);
        }
        self.set_key(key, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engine() -> Engine {
        let mut e = Engine::new(|key, _| key.to_owned());
        e.store_read(&json!({"incarnation":"inc-1","revision":"7","values":{}}));
        e.store_status(&json!({"role":"authority","stale":false}));
        e.take_effects();
        e
    }

    fn network(id: &str) -> Item {
        Item {
            id: id.into(),
            title: format!("Network {id}"),
            description: None,
            value: Some("Connect".into()),
            actions: vec![ItemAction {
                id: "connect".into(),
                label: "Connect".into(),
                enabled: true,
            }],
        }
    }

    #[test]
    fn live_items_are_data_found_by_search_and_listed_by_the_page() {
        let mut e = engine();
        e.set_items("wifi.visible", vec![network("home"), network("cafe")]);
        let hits = e.find("cafe");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, "wifi");
        assert_eq!(hits[0].item.as_deref(), Some("cafe"));
        let page = e.page_view(Some("wifi")).unwrap();
        let visible = page["sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == "visible")
            .unwrap();
        assert_eq!(visible["items"]["items"][1]["id"], "cafe");
    }

    #[test]
    fn an_item_action_on_a_page_without_its_backend_is_refused() {
        let mut e = engine();
        e.set_items("wifi.visible", vec![network("home")]);
        assert!(e.item_action("wifi.visible", "home", "connect").is_err());
        assert!(e.take_effects().is_empty());
    }

    #[test]
    fn prefs_set_operates_the_control_and_its_rules() {
        let mut e = engine();
        e.set_control("appearance.density", json!("compact")).unwrap();
        assert!(e.store.writing());
        // The pinned design source disables Mode for a person and an agent.
        e.store
            .values
            .insert("appearance.source".into(), json!({"design":"x"}));
        assert_eq!(
            e.set_control("appearance.mode", json!("light")).unwrap_err(),
            "row-appearance-mode-pinned"
        );
        // A key with no control on any page yet: settingsd's, not Prefs'.
        assert!(e.set_control("sound.output.amplify", json!(true)).is_err());
    }
}
