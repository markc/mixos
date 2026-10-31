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
    /// The row to bring into view and ring once; the view clears it.
    pub focus: Option<&'static str>,
    pub query: String,
    pub searching: bool,
    /// The sidebar overlay of the condensed window is open.
    pub sidebar: bool,
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
        }
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
        self.nav.focus = row;
        self.nav.query.clear();
        self.nav.searching = false;
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

    pub fn open_drawer(&mut self, drawer: &str) {
        if let Some(d) = pages::get(self.nav.page).and_then(|p| p.drawer(drawer)) {
            self.nav.drawer = Some(d.id);
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
        self.nav.searching = !query.is_empty() || self.nav.searching;
        self.nav.query = query;
    }

    pub fn close_search(&mut self) {
        self.nav.query.clear();
        self.nav.searching = false;
    }

    pub fn find(&self, query: &str) -> Vec<Hit> {
        pages::search(query, &|key| self.label(key, &[]))
    }

    /// Why `row` on `page` takes no input now, as a message; `None` when
    /// it does.
    pub fn row_blocked(&self, page: &pages::Page, row: &pages::Row) -> Option<String> {
        if let Some(backend) = page.backend.or(row.backend) {
            return Some(self.label("backend-missing", &[("backend", &self.label(backend, &[]))]));
        }
        if !row.enable.holds(&self.store) {
            return Some(self.label(row.why.unwrap_or("command-disabled"), &[]));
        }
        let writes = row.control.key().is_some() || matches!(row.control, Control::Reset { .. });
        if writes && let Some(blocked) = self.blocked() {
            return Some(self.label(blocked.label(), &[]));
        }
        None
    }

    /// Whether any of `keys` (a trailing `.*` names a prefix) is authored.
    pub fn authored(&self, keys: &[&str]) -> bool {
        self.store.values.keys().any(|path| {
            keys.iter().any(|k| match k.strip_suffix(".*") {
                Some(prefix) => path.starts_with(prefix) && path[prefix.len()..].starts_with('.'),
                None => path == k,
            })
        })
    }

    /// Reset every authored key among `keys`: one write of nulls.
    pub fn reset_keys(&mut self, keys: &[&str]) -> Result<(), String> {
        let paths: Vec<String> = self
            .store
            .values
            .keys()
            .filter(|path| {
                keys.iter().any(|k| match k.strip_suffix(".*") {
                    Some(prefix) => {
                        path.starts_with(prefix) && path[prefix.len()..].starts_with('.')
                    }
                    None => path == k,
                })
            })
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
        json!({"id": row.id, "title": self.label(&pages::row_title(page.id, row.id), &[]),
            "control": row.control, "key": key, "value": key.and_then(|k| self.store.value(k)),
            "shown": row.show.holds(&self.store), "blocked": self.row_blocked(page, row)})
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
                        "rows": s.rows.iter().map(|r| self.row_view(page, r)).collect::<Vec<_>>()})
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
                json!({"page": hit.page, "section": hit.section, "row": hit.row, "title": title})
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
        assert_eq!(e.nav.focus, Some("frosted_opacity"));
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
