// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The page framework: Preferences as data, in COSMIC Settings' shape.
//!
//! A [`Page`] is a sidebar entry or a sub-page. Its body is a list of
//! sub-pages, titled [`Section`]s of [`Row`]s, or an editor the shell draws
//! itself. Each row is one control bound to one settingsd key; its shape
//! (toggle, dropdown, slider, swatch…) and the key's range and choices come
//! from the page data and `settings.describe`. The renderer draws the data;
//! the Bus (`prefs.page.open`, `prefs.set`, `prefs.page`) and search read
//! the same data, so an agent reaches every control a person can.
//!
//! Labels follow the ids: a page's title is the message `page-<id>` and
//! its description `page-<id>-desc`; a section's title `sec-<page>-<id>`; a
//! row's title `row-<page>-<id>` and its description `row-<page>-<id>-desc`
//! when the catalogue has one.
//!
//! A row whose MixOS backend does not exist yet names it ([`Row::backend`],
//! [`Page::backend`]) and is drawn disabled with that line, never hidden.
use crate::store::Store;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::OnceLock;

mod appearance;
mod network;
mod panels;
mod tree;

/// A literal value in page data.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Lit {
    S(&'static str),
    I(i64),
    B(bool),
}

impl Lit {
    pub fn value(self) -> Value {
        match self {
            Lit::S(s) => json!(s),
            Lit::I(i) => json!(i),
            Lit::B(b) => json!(b),
        }
    }

    /// Whether `value` is this literal (an integer matches its float).
    pub fn is(self, value: Option<&Value>) -> bool {
        match (self, value) {
            (Lit::S(s), Some(Value::String(v))) => s == v,
            (Lit::I(i), Some(v)) => v.as_f64() == Some(i as f64),
            (Lit::B(b), Some(Value::Bool(v))) => b == *v,
            _ => false,
        }
    }
}

/// One choice of a dropdown, radio group, segmented control or image row.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Opt {
    pub value: Lit,
    /// The message id of its label.
    pub label: &'static str,
}

pub const fn opt(value: &'static str, label: &'static str) -> Opt {
    Opt {
        value: Lit::S(value),
        label,
    }
}

/// The lists a [`Control::Pick`] draws from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PickList {
    Fonts,
    MonoFonts,
    IconThemes,
}

/// How a choice is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChoiceStyle {
    Dropdown,
    /// One radio row per option (the row's title is the section's).
    Radio,
    Segmented,
    /// Image buttons with captions (roundness).
    Images,
}

/// A row's control.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "control", rename_all = "snake_case")]
pub enum Control {
    Toggle {
        key: &'static str,
    },
    /// A toggle over two values of an enum key (contrast normal/high).
    Flag {
        key: &'static str,
        on: Lit,
        off: Lit,
    },
    Choice {
        key: &'static str,
        style: ChoiceStyle,
        options: &'static [Opt],
    },
    /// A slider over a number; `scale` maps the stored value to the shown
    /// one (opacity 0–1 shown 0–100). Written on release.
    Slider {
        key: &'static str,
        min: f64,
        max: f64,
        step: f64,
        scale: f64,
        show_value: bool,
        start: Option<&'static str>,
        end: Option<&'static str>,
    },
    /// A slider over a fixed set of values (panel size), written on release.
    Steps {
        key: &'static str,
        steps: &'static [Lit],
        start: &'static str,
        end: &'static str,
    },
    /// Minus, value, plus.
    Spin {
        key: &'static str,
        min: i64,
        max: i64,
        step: i64,
    },
    /// A colour swatch that opens the colour drawer; `auto` resets to the
    /// key's `auto` word.
    Colour {
        key: &'static str,
    },
    /// The nine accent swatches and a custom one (`appearance.accent`).
    Accent,
    /// The colour schemes, the derived MixOS one first (`appearance.scheme`).
    Scheme,
    /// The Dark and Light image buttons (`appearance.mode`).
    Mode,
    /// The automatic light/dark switch (`appearance.mode = auto`).
    AutoMode,
    /// "Use the accent as the active window hint" (`window.hint_colour`).
    HintAccent,
    /// A name from a list the window gathers (font families, icon
    /// themes): a go-next row showing the current one, opening the list.
    Pick {
        key: &'static str,
        list: PickList,
    },
    /// A go-next row to another page.
    Page {
        page: &'static str,
    },
    /// A go-next row to one of this page's drawers.
    Drawer {
        drawer: &'static str,
    },
    /// A live value the backend owns (Wi-Fi, Bluetooth): read-only until
    /// the backend exists.
    Live {
        key: &'static str,
    },
    /// Read-only text: the row's description is the value.
    Info,
    /// "Reset to default", shown only while any of `keys` (a trailing `.*`
    /// covers a prefix) is authored.
    Reset {
        keys: &'static [&'static str],
    },
}

impl Control {
    /// The settingsd key the control writes, if one.
    pub fn key(&self) -> Option<&'static str> {
        match self {
            Control::Toggle { key }
            | Control::Flag { key, .. }
            | Control::Choice { key, .. }
            | Control::Slider { key, .. }
            | Control::Steps { key, .. }
            | Control::Spin { key, .. }
            | Control::Colour { key }
            | Control::Pick { key, .. }
            | Control::Live { key } => Some(key),
            Control::Accent => Some("appearance.accent"),
            Control::Scheme => Some("appearance.scheme"),
            Control::Mode | Control::AutoMode => Some("appearance.mode"),
            Control::HintAccent => Some("window.hint_colour"),
            Control::Page { .. }
            | Control::Drawer { .. }
            | Control::Info
            | Control::Reset { .. } => None,
        }
    }
}

/// When a row or section shows, or a row takes input.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cond {
    Always,
    Is(&'static str, Lit),
    Not(&'static str, Lit),
    /// The key holds a value (not null, not its default of null).
    Set(&'static str),
    Unset(&'static str),
    True(&'static str),
    False(&'static str),
}

impl Cond {
    pub fn holds(self, store: &Store) -> bool {
        match self {
            Cond::Always => true,
            Cond::Is(key, lit) => lit.is(store.value(key).as_ref()),
            Cond::Not(key, lit) => !lit.is(store.value(key).as_ref()),
            Cond::Set(key) => store.value(key).is_some_and(|v| !v.is_null()),
            Cond::Unset(key) => store.value(key).is_none_or(|v| v.is_null()),
            Cond::True(key) => store.flag(key),
            Cond::False(key) => !store.flag(key),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Row {
    pub id: &'static str,
    pub control: Control,
    /// Drawn only while this holds.
    pub show: Cond,
    /// Takes input only while every condition holds; else drawn disabled
    /// with the message of the first that fails.
    pub enable: &'static [(Cond, &'static str)],
    /// The missing MixOS backend, as a message id: the row is drawn
    /// disabled with a line naming it.
    pub backend: Option<&'static str>,
}

pub const fn row(id: &'static str, control: Control) -> Row {
    Row {
        id,
        control,
        show: Cond::Always,
        enable: &[],
        backend: None,
    }
}

impl Row {
    pub const fn show(mut self, cond: Cond) -> Self {
        self.show = cond;
        self
    }
    /// Enabled only while each `(condition, why)` holds.
    pub const fn enable(mut self, rules: &'static [(Cond, &'static str)]) -> Self {
        self.enable = rules;
        self
    }
    /// The message of the first enable rule that fails, if any.
    pub fn why_disabled(&self, store: &Store) -> Option<&'static str> {
        self.enable
            .iter()
            .find(|(cond, _)| !cond.holds(store))
            .map(|(_, why)| *why)
    }
    pub const fn backend(mut self, backend: &'static str) -> Self {
        self.backend = Some(backend);
        self
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Section {
    pub id: &'static str,
    /// Whether it has a heading (`sec-<page>-<id>`).
    pub titled: bool,
    pub rows: Vec<Row>,
    pub show: Cond,
}

pub fn section(id: &'static str, rows: Vec<Row>) -> Section {
    Section {
        id,
        titled: true,
        rows,
        show: Cond::Always,
    }
}

pub fn untitled(id: &'static str, rows: Vec<Row>) -> Section {
    Section {
        titled: false,
        ..section(id, rows)
    }
}

/// A right-hand drawer a go-next row opens: its own sections.
#[derive(Clone, Debug, Serialize)]
pub struct Drawer {
    pub id: &'static str,
    pub sections: Vec<Section>,
}

/// A header button beside the page title.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Extra {
    pub id: &'static str,
    /// The prefs command it runs; none while the work list holds it.
    pub command: Option<&'static str>,
    pub backend: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// A list of the sub-pages (Desktop, Input devices, …).
    List,
    Sections {
        sections: Vec<Section>,
        drawers: Vec<Drawer>,
    },
    /// An editor the shell draws itself (Software, the app pages).
    Custom { editor: &'static str },
    /// In the stage 5 work list, not built yet.
    Pending,
}

#[derive(Clone, Debug, Serialize)]
pub struct Page {
    pub id: &'static str,
    /// The Lucide icon.
    pub icon: &'static str,
    pub parent: Option<&'static str>,
    /// The missing backend of the whole page, as a message id.
    pub backend: Option<&'static str>,
    pub extras: Vec<Extra>,
    pub body: Body,
}

pub fn page(
    id: &'static str,
    icon: &'static str,
    parent: Option<&'static str>,
    body: Body,
) -> Page {
    Page {
        id,
        icon,
        parent,
        backend: None,
        extras: Vec::new(),
        body,
    }
}

pub fn sections(sections: Vec<Section>) -> Body {
    Body::Sections {
        sections,
        drawers: Vec::new(),
    }
}

impl Page {
    pub fn title(&self) -> String {
        format!("page-{}", self.id)
    }
    pub fn description(&self) -> String {
        format!("page-{}-desc", self.id)
    }
    pub fn sections(&self) -> &[Section] {
        match &self.body {
            Body::Sections { sections, .. } => sections,
            _ => &[],
        }
    }
    pub fn drawers(&self) -> &[Drawer] {
        match &self.body {
            Body::Sections { drawers, .. } => drawers,
            _ => &[],
        }
    }
    pub fn drawer(&self, id: &str) -> Option<&Drawer> {
        self.drawers().iter().find(|d| d.id == id)
    }
    /// Every row: the page's, then its drawers'.
    pub fn rows(&self) -> impl Iterator<Item = (&Section, &Row)> {
        self.sections()
            .iter()
            .chain(self.drawers().iter().flat_map(|d| d.sections.iter()))
            .flat_map(|s| s.rows.iter().map(move |r| (s, r)))
    }
    pub fn row(&self, id: &str) -> Option<&Row> {
        self.rows().map(|(_, r)| r).find(|r| r.id == id)
    }
    /// The drawer holding row `id`, if it sits in one.
    pub fn drawer_of(&self, id: &str) -> Option<&Drawer> {
        self.drawers()
            .iter()
            .find(|d| d.sections.iter().any(|s| s.rows.iter().any(|r| r.id == id)))
    }
}

pub fn row_title(page: &str, row: &str) -> String {
    format!("row-{page}-{row}")
}
pub fn row_description(page: &str, row: &str) -> String {
    format!("row-{page}-{row}-desc")
}
pub fn section_title(page: &str, section: &str) -> String {
    format!("sec-{page}-{section}")
}
pub fn drawer_title(page: &str, drawer: &str) -> String {
    format!("drawer-{page}-{drawer}")
}

/// Every page, in COSMIC's order: each top-level page followed by its
/// sub-pages.
pub fn all() -> &'static [Page] {
    static PAGES: OnceLock<Vec<Page>> = OnceLock::new();
    PAGES.get_or_init(tree::pages)
}

pub fn get(id: &str) -> Option<&'static Page> {
    all().iter().find(|p| p.id == id)
}

/// The sidebar: the pages with no parent.
pub fn top_level() -> impl Iterator<Item = &'static Page> {
    all().iter().filter(|p| p.parent.is_none())
}

pub fn children(id: &str) -> impl Iterator<Item = &'static Page> + '_ {
    all().iter().filter(move |p| p.parent == Some(id))
}

/// The top-level page `id` sits under (itself when it is one).
pub fn root(id: &str) -> &'static str {
    let mut page = get(id).map_or("desktop", |p| p.id);
    while let Some(parent) = get(page).and_then(|p| p.parent) {
        page = parent;
    }
    page
}

/// The start page.
pub const START: &str = "desktop";

/// One search hit: the page, and the row it lands on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Hit {
    pub page: &'static str,
    pub section: Option<&'static str>,
    pub row: Option<&'static str>,
}

/// Every page and row whose visible labels contain `query` (case-folded,
/// whitespace literal): page titles and descriptions, section headings, row
/// titles, descriptions and option labels. A row hit lands on its control.
pub fn search(query: &str, label: &dyn Fn(&str) -> String) -> Vec<Hit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let found = |key: &str| {
        let text = label(key);
        text != key && text.to_lowercase().contains(&query)
    };
    let mut hits = Vec::new();
    for page in all() {
        let mut any_row = false;
        for (section, row) in page.rows() {
            let options: &[Opt] = match &row.control {
                Control::Choice { options, .. } => options,
                _ => &[],
            };
            if found(&row_title(page.id, row.id))
                || found(&row_description(page.id, row.id))
                || (section.titled && found(&section_title(page.id, section.id)))
                || options.iter().any(|o| found(o.label))
            {
                any_row = true;
                hits.push(Hit {
                    page: page.id,
                    section: Some(section.id),
                    row: Some(row.id),
                });
            }
        }
        if !any_row && (found(&page.title()) || found(&page.description())) {
            hits.push(Hit {
                page: page.id,
                section: None,
                row: None,
            });
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn page_ids_are_unique_and_parents_exist() {
        let mut seen = BTreeSet::new();
        for page in all() {
            assert!(seen.insert(page.id), "duplicate page {}", page.id);
            if let Some(parent) = page.parent {
                let parent = get(parent).unwrap_or_else(|| panic!("{}: no parent", page.id));
                assert!(
                    all().iter().position(|p| p.id == parent.id)
                        < all().iter().position(|p| p.id == page.id),
                    "{} comes after its parent",
                    page.id
                );
            }
            let mut rows = BTreeSet::new();
            for (_, row) in page.rows() {
                assert!(rows.insert(row.id), "{}: duplicate row {}", page.id, row.id);
            }
        }
    }

    #[test]
    fn the_sidebar_is_cosmic_s_eleven_in_order() {
        let ids: Vec<_> = top_level().map(|p| p.id).collect();
        assert_eq!(
            ids,
            [
                "network",
                "bluetooth",
                "accessibility",
                "desktop",
                "displays",
                "sound",
                "power",
                "input",
                "applications",
                "time",
                "system"
            ]
        );
        assert_eq!(root("panel_applets"), "desktop");
        assert_eq!(root("wifi"), "network");
    }

    #[test]
    fn every_bound_key_is_in_the_registry() {
        let catalog = crate::catalog::Catalog::builtin();
        for page in all() {
            for (_, row) in page.rows() {
                if let Some(key) = row.control.key() {
                    assert!(catalog.get(key).is_some(), "{}/{}: {key}", page.id, row.id);
                }
                if let Control::Choice { key, options, .. } = &row.control
                    && let Some(crate::catalog::Kind::Enum(variants)) =
                        catalog.get(key).map(|i| i.kind.clone())
                {
                    for o in *options {
                        if let Lit::S(s) = o.value {
                            assert!(variants.iter().any(|v| v == s), "{key}: {s}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn search_lands_on_rows_and_pages() {
        let label = |key: &str| match key {
            "row-appearance-density" => "Interface density".to_owned(),
            "page-wifi" => "Wi-Fi".to_owned(),
            other => other.to_owned(),
        };
        let hits = search("DENSITY", &label);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, "appearance");
        assert_eq!(hits[0].row, Some("density"));
        assert_eq!(search("wi-fi", &label)[0].page, "wifi");
        assert!(search("  ", &label).is_empty());
    }
}
