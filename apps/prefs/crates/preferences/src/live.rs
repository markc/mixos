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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Item {
    /// Stable within its source (an SSID, a connector, a device address).
    pub id: String,
    /// A Lucide icon name (a card's 48 px device icon, a row's leading
    /// icon).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
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
    values: BTreeMap<String, String>,
}

impl Engine {
    /// The owner of `source` replaces its list.
    pub fn set_items(&mut self, source: &str, items: Vec<Item>) {
        self.live.items.insert(source.to_owned(), items);
        self.refresh_hits();
    }

    /// The owner of the data reports a presence fact (`power.battery`):
    /// rows and sections shown on [`pages::Cond::Fact`] follow it.
    pub fn set_fact(&mut self, name: &str, present: bool) {
        self.store.facts.insert(name.to_owned(), present);
        self.refresh_hits();
    }

    /// A choice option's one-line description, from the page's option.
    pub fn choice_description(&self, row: &pages::Row, value: &Value) -> Option<String> {
        let pages::Control::Choice { options, .. } = &row.control else {
            return None;
        };
        options
            .iter()
            .find(|o| o.value.value() == *value)
            .and_then(|o| o.description)
            .map(|d| self.label(d, &[]))
    }

    /// The owner of `source` reports its read-only value (the current
    /// time, a battery's "72% (2 h left)"), shown on rows with
    /// [`pages::Row::value_from`].
    pub fn set_value(&mut self, source: &str, text: String) {
        self.live.values.insert(source.to_owned(), text);
    }

    pub fn live_value(&self, source: &str) -> Option<&str> {
        self.live.values.get(source).map(String::as_str)
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
            .find(|p| {
                p.sections()
                    .iter()
                    .any(|s| s.items.is_some_and(|i| i.source == source))
            })
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
            .filter(|o| o.listed)
            .filter(|o| match (variants, o.value) {
                (Some(names), pages::Lit::S(s)) => names.iter().any(|n| n == s),
                _ => true,
            })
            .map(|o| (o.value.value(), self.label(o.label, &[])))
            .collect();
        for name in variants.into_iter().flatten() {
            // A variant the page names (offered or not) is the page's call.
            let paged = options
                .iter()
                .any(|o| matches!(o.value, pages::Lit::S(s) if s == name));
            if !paged && !out.iter().any(|(v, _)| v.as_str() == Some(name)) {
                let key = format!("opt-{}", name.replace('_', "-"));
                let text = self.label(&key, &[]);
                out.push((
                    Value::from(name.as_str()),
                    if text == key { name.clone() } else { text },
                ));
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

    /// The entries of a list key, in order.
    pub fn entries(&self, key: &str) -> Vec<Value> {
        self.store
            .value(key)
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
    }

    /// How many entries a list key takes: the catalogue's bounds.
    pub fn list_bounds(&self, key: &str) -> (usize, usize) {
        match self.store.catalog.get(key).map(|i| &i.kind) {
            Some(crate::catalog::Kind::List { min, max }) => (*min, *max),
            _ => (0, usize::MAX),
        }
    }

    /// An entry's label: the first of `sources` that offers it, else its
    /// parts joined (`de (nodeadkeys)`).
    pub fn entry_label(&self, sources: &[&str], value: &Value) -> String {
        sources
            .iter()
            .flat_map(|s| self.live_options(s))
            .find(|(v, _)| v == value)
            .map(|(_, text)| text.clone())
            .unwrap_or_else(|| raw_label(value))
    }

    /// The label a choice row shows for a stored `value` it does not offer
    /// (an unlisted option).
    pub fn unlisted_label(&self, row: &pages::Row, value: &Value) -> Option<String> {
        let pages::Control::Choice { options, .. } = &row.control else {
            return None;
        };
        options
            .iter()
            .find(|o| !o.listed && o.value.is(Some(value)))
            .map(|o| self.label(o.label, &[]))
    }

    /// Select the output tab `output` of a [`pages::Control::Outputs`]
    /// (none: back to the default).
    pub fn select_output(&mut self, output: Option<String>) {
        self.nav.output = output;
    }

    /// The output tab selected: the one chosen, else the internal display,
    /// else the first connected one (spec 1 §4.1).
    pub fn selected_output(&self) -> Option<String> {
        let outputs: Vec<&str> = self
            .live_options("outputs")
            .iter()
            .filter_map(|(v, _)| v.as_str())
            .filter(|v| *v != "all")
            .collect();
        if let Some(output) = &self.nav.output
            && outputs.contains(&output.as_str())
        {
            return Some(output.clone());
        }
        outputs
            .iter()
            .find(|o| ["eDP-", "LVDS-", "DSI-"].iter().any(|p| o.starts_with(p)))
            .or(outputs.first())
            .map(|o| (*o).to_owned())
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
                .find(|row| row.control.keys().contains(&key))
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
        self.write_key(key, value)
    }

    /// The write a control sends for `value` on `key`: its row's
    /// [`pages::Row::writes`] when it has one, else the key alone.
    pub(crate) fn control_writes(&self, key: &str, value: Value) -> BTreeMap<String, Value> {
        match self.control_of(key).and_then(|(_, row)| row.writes) {
            Some(writes) => writes(self, key, &value),
            None => BTreeMap::from([(key.to_owned(), value)]),
        }
    }

    /// Write `value` to `key` as its control does: the control's whole
    /// write in one fenced apply, through the store's one write path
    /// (checked, merged with pending writes, fenced, re-read on conflict,
    /// kept while the authority is unreachable). The window's path: its
    /// rows already refuse what their rules disable; `prefs.set` checks
    /// them first ([`Self::set_control`]).
    pub fn write_key(&mut self, key: &str, value: Value) -> Result<(), String> {
        let changes = self.control_writes(key, value);
        self.set_keys(changes)
    }

}

/// A value as plain text: a string as itself, a list's non-empty parts
/// as `first (rest, …)`.
fn raw_label(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let parts: Vec<String> = parts
                .iter()
                .map(raw_label)
                .filter(|p| !p.is_empty())
                .collect();
            match parts.split_first() {
                Some((first, [])) => first.clone(),
                Some((first, rest)) => format!("{first} ({})", rest.join(", ")),
                None => String::new(),
            }
        }
        other => other.to_string(),
    }
}

/// One edit of an ordered list.
#[derive(Clone, Debug, PartialEq)]
pub enum ListEdit {
    /// Move entry `index` one place up (towards the start).
    Up(usize),
    Down(usize),
    Remove(usize),
    Append(Value),
}

impl ListEdit {
    /// `list` after the edit; `None` when it changes nothing (the first
    /// moved up, an entry already there appended).
    pub fn apply(&self, list: &[Value]) -> Option<Vec<Value>> {
        let mut out = list.to_vec();
        match self {
            ListEdit::Up(i) if *i > 0 && *i < out.len() => out.swap(i - 1, *i),
            ListEdit::Down(i) if i + 1 < out.len() => out.swap(*i, i + 1),
            ListEdit::Remove(i) if *i < out.len() => {
                out.remove(*i);
            }
            ListEdit::Append(value) if !out.contains(value) => out.push(value.clone()),
            _ => return None,
        }
        Some(out)
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
            value: Some("Connect".into()),
            actions: vec![ItemAction {
                id: "connect".into(),
                label: "Connect".into(),
                enabled: true,
            }],
            ..Item::default()
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
    fn a_fact_shows_what_its_backend_reports_and_hides_it_until_then() {
        let mut e = engine();
        let cond = pages::Cond::Fact("power.battery");
        let none = pages::Cond::NoFact("power.battery");
        assert!(!cond.holds(&e.store) && !none.holds(&e.store), "unreported");
        e.set_fact("power.battery", false);
        assert!(!cond.holds(&e.store) && none.holds(&e.store));
        e.set_fact("power.battery", true);
        assert!(cond.holds(&e.store));
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
        e.set_control("appearance.density", json!("compact"))
            .unwrap();
        assert!(e.store.writing());
        // The pinned design source disables Mode for a person and an agent.
        e.store
            .values
            .insert("appearance.source".into(), json!({"design":"x"}));
        assert_eq!(
            e.set_control("appearance.mode", json!("light"))
                .unwrap_err(),
            "row-appearance-mode-pinned"
        );
        // A key with no control on any page yet: settingsd's, not Prefs'.
        assert!(e.set_control("sound.output.amplify", json!(true)).is_err());
    }

    #[test]
    fn a_control_write_is_one_fenced_apply_through_the_store() {
        let mut e = engine();
        e.write_key("appearance.density", json!("compact")).unwrap();
        e.write_key("appearance.roundness", json!("square")).unwrap();
        let applies: Vec<Value> = e
            .take_effects()
            .into_iter()
            .filter_map(|x| match x {
                Effect::Store { command: "settings.apply", body, .. } => Some(body),
                _ => None,
            })
            .collect();
        // The first goes at once; the second waits for its answer, merged.
        assert_eq!(applies.len(), 1);
        assert_eq!(applies[0]["changes"], json!({"appearance.density":"compact"}));
        assert_eq!(e.store.waiting(), ["appearance.roundness", "appearance.density"]);
        // prefs.set takes the same path after the control's rules.
        e.store.values.insert("appearance.source".into(), json!({"design":"x"}));
        assert!(e.set_control("appearance.mode", json!("light")).is_err());
    }

    #[test]
    fn the_output_tab_defaults_to_the_internal_display_and_follows_a_choice() {
        let mut e = engine();
        assert_eq!(e.selected_output(), None);
        e.set_options(
            "outputs",
            vec![
                (json!("all"), "All".into()),
                (json!("HDMI-A-1"), "HDMI".into()),
                (json!("eDP-1"), "Laptop".into()),
            ],
        );
        assert_eq!(e.selected_output().as_deref(), Some("eDP-1"));
        e.select_output(Some("HDMI-A-1".into()));
        assert_eq!(e.selected_output().as_deref(), Some("HDMI-A-1"));
        // A chosen output that went away falls back to the default.
        e.select_output(Some("DP-9".into()));
        assert_eq!(e.selected_output().as_deref(), Some("eDP-1"));
    }

    #[test]
    fn a_readout_shows_its_owner_s_latest_value() {
        let mut e = engine();
        assert_eq!(e.live_value("time.now"), None);
        e.set_value("time.now", "10:42".into());
        e.set_value("time.now", "10:43".into());
        assert_eq!(e.live_value("time.now"), Some("10:43"));
    }

    #[test]
    fn list_edits_move_remove_and_append_once() {
        let list = [json!(["us", ""]), json!(["de", "nodeadkeys"])];
        assert_eq!(
            ListEdit::Up(1).apply(&list).unwrap()[0],
            json!(["de", "nodeadkeys"])
        );
        assert_eq!(ListEdit::Up(0).apply(&list), None);
        assert_eq!(ListEdit::Down(1).apply(&list), None);
        assert_eq!(
            ListEdit::Remove(0).apply(&list).unwrap(),
            [json!(["de", "nodeadkeys"])]
        );
        assert_eq!(ListEdit::Append(json!(["us", ""])).apply(&list), None);
        assert_eq!(
            ListEdit::Append(json!(["fr", ""]))
                .apply(&list)
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn an_entry_is_labelled_from_its_source_or_its_parts() {
        let mut e = engine();
        e.set_options(
            "xkb.layouts",
            vec![(json!(["us", ""]), "English (US)".into())],
        );
        assert_eq!(
            e.entry_label(&["xkb.layouts"], &json!(["us", ""])),
            "English (US)"
        );
        assert_eq!(
            e.entry_label(&["xkb.layouts"], &json!(["de", "nodeadkeys"])),
            "de (nodeadkeys)"
        );
        assert_eq!(e.entries("input.keyboard.layouts"), [json!(["us", ""])]);
        assert_eq!(e.list_bounds("input.keyboard.layouts"), (1, 8));
    }
}
