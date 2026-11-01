// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Where the window is: the page, the drawer open on it, the row a search
//! or `prefs.page.open {row}` landed on, the search query and the condensed
//! sidebar. There is no back stack: back goes to the page's parent
//! (spec 4, "Back navigation").
use crate::Section;
use crate::engine::Engine;
use crate::pages::{self, Body, Control, Hit};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Nav {
    pub page: &'static str,
    pub drawer: Option<&'static str>,
    /// The row to bring into view and ring once, as (page, row): row ids
    /// are unique only within a page. The view clears it.
    pub focus: Option<(&'static str, &'static str)>,
    pub query: String,
    pub searching: bool,
    /// The sidebar overlay of the condensed window is open.
    pub sidebar: bool,
    /// The output tab chosen on an outputs selector (the window's, not a
    /// setting); none is the default tab.
    pub output: Option<String>,
    /// The hits for `query`, kept until the query or the values change, so
    /// a repaint never searches.
    #[serde(skip)]
    pub hits: Vec<Hit>,
}

impl Default for Nav {
    fn default() -> Self {
        Self {
            page: pages::START,
            drawer: None,
            focus: None,
            query: String::new(),
            searching: false,
            sidebar: false,
            output: None,
            hits: Vec::new(),
        }
    }
}

/// Whether `pattern` names `path`: the same path, or, ending `.*`, any
/// path under that prefix (or the prefix itself).
pub fn covers(pattern: &str, path: &str) -> bool {
    match pattern.strip_suffix(".*") {
        Some(prefix) => {
            path == prefix
                || path
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('.'))
        }
        None => path == pattern,
    }
}

/// The page that shows an old section, for the legacy `prefs.section`.
pub fn page_of(section: Section) -> &'static str {
    match section {
        Section::Appearance | Section::TextAndFonts => "appearance",
        Section::Wallpapers => "wallpaper",
        Section::Windows => "window_management",
        Section::Email => "email",
        Section::Terminal => "term",
        Section::Ced => "ced",
        Section::Accessibility => "accessibility",
        Section::Software => "software",
    }
}

/// The old editor a custom page draws.
fn editor_section(editor: &str) -> Option<Section> {
    Some(match editor {
        "wallpaper" => Section::Wallpapers,
        "term" => Section::Terminal,
        "email" => Section::Email,
        "ced" => Section::Ced,
        "software" => Section::Software,
        _ => return None,
    })
}

/// A page id as a person or a deep link spells it (`window-management`).
pub fn page_id(name: &str) -> Option<&'static str> {
    let name = name.trim().replace('-', "_");
    pages::get(&name).map(|p| p.id)
}

impl Engine {
    /// Open `page`, landing on `row` when given; closes search and any
    /// drawer. A row in one of the page's drawers opens that drawer.
    pub fn open_page(&mut self, page: &str, row: Option<&str>) -> Result<(), String> {
        let id = page_id(page).ok_or_else(|| format!("{page}: no such page"))?;
        let page = pages::get(id).expect("a known page");
        let row = match row {
            Some(row) => Some(
                page.row(row)
                    .map(|r| r.id)
                    .ok_or_else(|| format!("{}: no row {row}", page.id))?,
            ),
            None => None,
        };
        if !self.can_navigate() {
            return Err(self.label("command-disabled", &[]));
        }
        if let Body::Custom { editor } = page.body
            && let Some(section) = editor_section(editor)
        {
            self.set_section(section);
        }
        self.nav.page = id;
        self.nav.drawer = row.and_then(|r| page.drawer_of(r)).map(|d| d.id);
        // A hidden row has nowhere to land: open the page only.
        self.nav.focus = row
            .filter(|r| page.row(r).is_some_and(|r| r.show.holds(&self.store)))
            .map(|r| (id, r));
        self.close_search();
        self.nav.sidebar = false;
        Ok(())
    }

    /// The sub-page's back link: its parent page.
    pub fn back(&mut self) {
        if let Some(parent) = pages::get(self.nav.page).and_then(|p| p.parent) {
            let _ = self.open_page(parent, None);
        }
    }

    pub fn can_back(&self) -> bool {
        self.can_navigate() && pages::get(self.nav.page).is_some_and(|p| p.parent.is_some())
    }

    /// Open one of the page's drawers, or a row's own drawer (a colour
    /// swatch's picker, the accent's custom colour, a pick row's list),
    /// named by the row's id.
    pub fn open_drawer(&mut self, drawer: &str) {
        let Some(page) = pages::get(self.nav.page) else {
            return;
        };
        if let Some(d) = page.drawer(drawer) {
            self.nav.drawer = Some(d.id);
        } else if let Some(row) = page.row(drawer)
            && matches!(
                row.control,
                Control::Colour { .. } | Control::Accent | Control::Pick { .. }
            )
        {
            self.nav.drawer = Some(row.id);
        }
    }

    pub fn close_drawer(&mut self) {
        self.nav.drawer = None;
    }

    pub fn toggle_sidebar(&mut self) {
        self.nav.sidebar = !self.nav.sidebar;
    }

    pub fn open_search(&mut self) {
        self.nav.searching = true;
        self.ui.search_focus = true;
    }

    /// The live query; emptying it closes search, back to the page.
    pub fn set_query(&mut self, query: String) {
        // Typed with nothing focused: the field takes the rest.
        if !self.nav.searching && !query.is_empty() {
            self.ui.search_focus = true;
        }
        self.nav.searching = !query.is_empty() || self.nav.searching;
        self.nav.query = query;
        self.refresh_hits();
    }

    pub fn close_search(&mut self) {
        self.nav.query.clear();
        self.nav.searching = false;
        self.nav.hits.clear();
    }

    /// Recompute the cached hits (the query or the values moved).
    pub fn refresh_hits(&mut self) {
        if self.nav.searching {
            self.nav.hits = self.find(&self.nav.query);
        }
    }

    /// The rows (and live items) whose visible labels contain `query`,
    /// skipping rows hidden now (their `show` fails).
    pub fn find(&self, query: &str) -> Vec<Hit> {
        let index = self
            .index
            .get_or_init(|| pages::Index::new(&|key| self.label(key, &[])));
        let mut hits: Vec<Hit> = index
            .search(query)
            .into_iter()
            .filter(|hit| match (pages::get(hit.page), hit.row) {
                (Some(page), Some(row)) => page.row(row).is_some_and(|r| r.show.holds(&self.store)),
                _ => true,
            })
            .collect();
        hits.extend(self.find_items(query));
        hits
    }

    /// Why the whole of `page` takes no input now (its backend is missing,
    /// or its writes are blocked), as a message: drawn once under the
    /// title, and never again on its rows (Fable P1).
    pub fn page_reason(&self, page: &pages::Page) -> Option<String> {
        if let Some(backend) = page.backend {
            return Some(self.label("backend-missing", &[("backend", &self.label(backend, &[]))]));
        }
        let writes = page.rows().any(|(_, r)| r.control.key().is_some());
        match self.blocked() {
            Some(blocked) if writes => Some(self.label(blocked.label(), &[])),
            _ => None,
        }
    }

    /// The reason a row draws under its own labels: its own, only when the
    /// page as a whole is live.
    pub fn row_reason(&self, page: &pages::Page, row: &pages::Row) -> Option<String> {
        if self.page_reason(page).is_some() {
            return None;
        }
        self.row_blocked(page, row)
    }

    /// Why `row` on `page` takes no input now, as a message; `None` when
    /// it does.
    pub fn row_blocked(&self, page: &pages::Page, row: &pages::Row) -> Option<String> {
        if let Some(backend) = page.backend.or(row.backend) {
            return Some(self.label("backend-missing", &[("backend", &self.label(backend, &[]))]));
        }
        if let Some(why) = row.why_disabled(&self.store) {
            return Some(self.label(why, &[]));
        }
        let writes = row.control.key().is_some() || matches!(row.control, Control::Reset { .. });
        if writes && let Some(blocked) = self.blocked() {
            return Some(self.label(blocked.label(), &[]));
        }
        None
    }

    /// Whether any of `keys` (a trailing `.*` names a prefix) is authored.
    pub fn authored(&self, keys: &[&str]) -> bool {
        self.store
            .values
            .keys()
            .any(|path| keys.iter().any(|k| covers(k, path)))
    }

    /// Reset every authored key among `keys`: one write of nulls.
    pub fn reset_keys(&mut self, keys: &[&str]) -> Result<(), String> {
        let paths: Vec<String> = self
            .store
            .values
            .keys()
            .filter(|path| keys.iter().any(|k| covers(k, path)))
            .cloned()
            .collect();
        if paths.is_empty() {
            return Ok(());
        }
        self.set_keys(paths.into_iter().map(|p| (p, Value::Null)).collect())
    }

    /// One row as data: its control, the key's value, and whether (and
    /// why not) it takes input.
    pub fn row_view(&self, page: &pages::Page, row: &pages::Row) -> Value {
        let key = row.control.key();
        let mut view = json!({"id": row.id, "title": self.label(&pages::row_title(page.id, row.id), &[]),
            "control": row.control, "key": key, "value": key.and_then(|k| self.store.value(k)),
            "shown": row.show.holds(&self.store), "blocked": self.row_blocked(page, row)});
        if let Some(source) = row.value_from {
            view["readout"] = json!(self.live_value(source));
        }
        // An ordered list's entries as shown, in order.
        if let Control::Ordered { key, sources, .. } = row.control {
            view["entries"] = self
                .entries(key)
                .iter()
                .map(|v| json!({"value": v, "label": self.entry_label(sources, v)}))
                .collect();
        }
        view
    }

    /// `prefs.page`: the open page (or `page`) with every section and row.
    pub fn page_view(&self, page: Option<&str>) -> Result<Value, String> {
        let id = match page {
            Some(name) => page_id(name).ok_or_else(|| format!("{name}: no such page"))?,
            None => self.nav.page,
        };
        let page = pages::get(id).expect("a known page");
        let sections = |sections: &[pages::Section]| -> Vec<Value> {
            sections
                .iter()
                .map(|s| {
                    json!({"id": s.id, "title": s.titled.then(|| self.label(&pages::section_title(page.id, s.id), &[])),
                        "shown": s.show.holds(&self.store),
                        "rows": s.rows.iter().map(|r| self.row_view(page, r)).collect::<Vec<_>>(),
                        "items": s.items.map(|i| json!({"source": i.source, "items": self.items(i.source)}))})
                })
                .collect()
        };
        let drawers: Vec<Value> = page
            .drawers()
            .iter()
            .map(|d| json!({"id": d.id, "sections": sections(&d.sections)}))
            .collect();
        let kind = match page.body {
            Body::List => "list",
            Body::Sections { .. } => "sections",
            Body::Custom { .. } => "custom",
            Body::Pending => "pending",
        };
        Ok(
            json!({"page": page.id, "title": self.label(&page.title(), &[]), "parent": page.parent,
            "kind": kind, "backend": page.backend.map(|b| self.label(b, &[])),
            "children": pages::children(page.id).map(|p| p.id).collect::<Vec<_>>(),
            "sections": sections(page.sections()), "drawers": drawers,
            "open": self.nav.page == page.id, "drawer": self.nav.drawer, "store": self.store_view()}),
        )
    }

    /// `prefs.pages`: the tree, in sidebar order.
    pub fn pages_view(&self) -> Value {
        let pages: Vec<Value> = pages::all()
            .iter()
            .map(|p| {
                let kind = match p.body {
                    Body::List => "list",
                    Body::Sections { .. } => "sections",
                    Body::Custom { .. } => "custom",
                    Body::Pending => "pending",
                };
                json!({"id": p.id, "title": self.label(&p.title(), &[]), "parent": p.parent,
                    "icon": p.icon, "kind": kind, "backend": p.backend})
            })
            .collect();
        json!({"pages": pages, "open": self.nav.page})
    }

    /// `prefs.search {query}`: the hits, each landing on a row.
    pub fn search_view(&self, query: &str) -> Value {
        let hits: Vec<Value> = self
            .find(query)
            .into_iter()
            .map(|hit| {
                let title = match (hit.row, pages::get(hit.page)) {
                    (Some(row), Some(page)) => self.label(&pages::row_title(page.id, row), &[]),
                    (None, Some(page)) => self.label(&page.title(), &[]),
                    _ => String::new(),
                };
                let title = match &hit.item {
                    Some(id) => self
                        .items_of_page(hit.page)
                        .find(|item| &item.id == id)
                        .map_or(title, |item| item.title.clone()),
                    None => title,
                };
                json!({"page": hit.page, "section": hit.section, "row": hit.row, "item": hit.item, "title": title})
            })
            .collect();
        json!({"query": query, "hits": hits})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        let mut e = Engine::new(|key, _| key.to_owned());
        e.take_effects();
        e
    }

    #[test]
    fn deep_links_take_cosmic_s_spelling() {
        assert_eq!(page_id("window-management"), Some("window_management"));
        assert_eq!(page_id("wifi"), Some("wifi"));
        assert_eq!(page_id("nowhere"), None);
    }

    #[test]
    fn opening_a_drawer_row_opens_its_drawer_and_back_goes_to_the_parent() {
        let mut e = engine();
        e.open_page("appearance", Some("frosted_opacity")).unwrap();
        assert_eq!(e.nav.page, "appearance");
        assert_eq!(e.nav.drawer, Some("frosted"));
        assert_eq!(e.nav.focus, Some(("appearance", "frosted_opacity")));

        e.back();
        assert_eq!(e.nav.page, "desktop");
        assert_eq!(e.nav.drawer, None);
        assert!(e.open_page("appearance", Some("nope")).is_err());
    }

    #[test]
    fn a_missing_backend_disables_its_rows_with_the_backend_named() {
        let e = engine();
        let wifi = pages::get("wifi").unwrap();
        let toggle = wifi.row("enabled").unwrap();
        assert!(
            e.row_blocked(wifi, toggle)
                .unwrap()
                .contains("backend-missing")
        );
    }

    #[test]
    fn auto_mode_waits_for_a_zone_with_a_sunrise() {
        let mut e = engine();
        let page = pages::get("appearance").unwrap();
        let auto = page.row("auto").unwrap();
        assert_eq!(
            e.row_blocked(page, auto).as_deref(),
            Some("row-appearance-auto-no-daylight")
        );
        e.store
            .values
            .insert("state.appearance.daylight".into(), json!(false));
        // Daylight known: only the unread store holds it now.
        assert_eq!(e.row_blocked(page, auto).as_deref(), Some("store-unread"));
    }

    #[test]
    fn a_pinned_design_source_disables_mode_with_its_notice() {
        let mut e = engine();
        e.store
            .values
            .insert("appearance.source".into(), json!({"design": "x"}));
        let page = pages::get("appearance").unwrap();
        // Unread, so writes are blocked anyway; the pin speaks first.
        assert_eq!(
            e.row_blocked(page, page.row("mode").unwrap()).as_deref(),
            Some("row-appearance-mode-pinned")
        );
    }
}
