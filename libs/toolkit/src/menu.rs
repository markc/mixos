// SPDX-License-Identifier: MIT OR Apache-2.0
//! Menu bars and their drop-down menus (chrome specification §3.3–3.6).
//!
//! The toolkit draws its menus itself rather than through egui's `MenuBar`,
//! because the specified behaviour needs things egui's menus do not do: one
//! highlight shared by pointer and keyboard, menus that open on press and
//! run an item on a press-drag-release, and hover switching between titles
//! that a resting pointer cannot undo.
//!
//! - [`Menu`], [`Entry`], [`Row`]: the model, built each frame (the command
//!   registry builds it from its commands, [`crate::command::Registry::menus`]).
//! - [`Nav`]: which menu is open and which row of each level is highlighted,
//!   with the keyboard rules of §3.6. Pure data, tested without egui.
//! - [`bar`]: draws the titles into the current row and each open level as a
//!   foreground area, and returns the command chosen this frame.
//!
//! Pointer rules: a press on a closed title opens its menu at once, and the
//! release that ends that press never counts as a click outside; releasing
//! over an enabled row (after a press-drag or a click) runs it; a press
//! outside every menu closes them; a click inside on anything but an enabled
//! row closes them too. While a menu is open, moving the pointer onto another
//! title opens that one instead, and moving onto a row highlights it.
//!
//! One menu bar per window: its state lives in the context under one id, so
//! [`is_open`] can hold back global shortcuts while a menu has the keyboard.

use crate::chrome::{Chrome, DISABLED_ALPHA, WEAK_ALPHA};
use egui::emath::GuiRounding;
use egui::{
    Area, Color32, Context, Frame, Id, Key, Modifiers, Order, PointerButton, Pos2, Rect, ScrollArea, Sense, Stroke,
    TextEdit, Ui, UiKind, Vec2, WidgetInfo, WidgetType, pos2, text::Galley, vec2,
};
use std::sync::Arc;

/// The stock submenu arrow, in the shortcut column (§3.5).
pub const SUBMENU_ARROW: &str = "⏵";

/// The tick of a ticked choice row, in a gutter before the label (§3.5:
/// "✔" and a space; every label of a level with choices indents by it).
pub const TICK: &str = "✔";
const TICK_GUTTER: &str = "✔ ";

/// One line of a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Row(Row),
    /// A hairline between groups (§3.5).
    Separator,
}

impl Entry {
    pub fn row(&self) -> Option<&Row> {
        match self {
            Self::Row(row) => Some(row),
            Self::Separator => None,
        }
    }
}

/// A command row, or a row that opens a submenu.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The command run by this row; `None` for a submenu row.
    pub id: Option<&'static str>,
    pub label: String,
    /// Already formatted for the platform ("Ctrl+Alt+Shift+O").
    pub shortcut: Option<String>,
    pub enabled: bool,
    /// A choice row's tick (§3.5, check rows): `Some(true)` ticked,
    /// `Some(false)` not; `None` for a row that is no choice.
    pub checked: Option<bool>,
    /// The submenu's entries; empty for a command row.
    pub children: Vec<Entry>,
}

impl Row {
    pub fn command(id: &'static str, label: impl Into<String>, shortcut: Option<String>, enabled: bool) -> Self {
        Self { id: Some(id), label: label.into(), shortcut, enabled, checked: None, children: Vec::new() }
    }

    /// A choice row, ticked when `checked`.
    pub fn choice(id: &'static str, label: impl Into<String>, shortcut: Option<String>, enabled: bool, checked: bool) -> Self {
        Self { checked: Some(checked), ..Self::command(id, label, shortcut, enabled) }
    }

    /// A submenu row: enabled when any of its children is (§3.5).
    pub fn submenu(label: impl Into<String>, children: Vec<Entry>) -> Self {
        let enabled = children.iter().filter_map(Entry::row).any(|r| r.enabled);
        Self { id: None, label: label.into(), shortcut: None, enabled, checked: None, children: tidy(children) }
    }

    pub fn is_submenu(&self) -> bool {
        !self.children.is_empty()
    }
}

/// A top-level menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub title: String,
    pub entries: Vec<Entry>,
}

/// `entries` with every separator that would come first, last or straight
/// after another removed (§3.5).
pub fn tidy(entries: Vec<Entry>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry == Entry::Separator && matches!(out.last(), None | Some(Entry::Separator)) {
            continue;
        }
        out.push(entry);
    }
    if out.last() == Some(&Entry::Separator) {
        out.pop();
    }
    out
}

/// A search field at the top of one top-level menu (§3.5: Help).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Search {
    /// The index of the menu that carries it.
    pub menu: usize,
    pub hint: String,
    /// Shown, weak, when nothing matches.
    pub empty: String,
}

/// The most results a search lists (§3.5).
pub const SEARCH_RESULTS: usize = 15;

/// Joins a result's menu path (§3.5).
pub const PATH_SEPARATOR: &str = " › ";

/// The search field's width (§3.5) and the separator band after it.
const SEARCH_WIDTH: f32 = 220.0;

/// The command rows of `menus` matching `query` (case-insensitive), each
/// labelled with its menu path joined by [`PATH_SEPARATOR`], ranked: the
/// label starts with the query, then a word of it does, then the label
/// contains it, then the path does (§3.5). At most [`SEARCH_RESULTS`].
pub fn matching(menus: &[Menu], query: &str) -> Vec<Row> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    fn walk<'a>(entries: &'a [Entry], path: &mut Vec<&'a str>, out: &mut Vec<(Vec<&'a str>, &'a Row)>) {
        for row in entries.iter().filter_map(Entry::row) {
            if row.is_submenu() {
                path.push(&row.label);
                walk(&row.children, path, out);
                path.pop();
            } else if row.id.is_some() {
                out.push((path.clone(), row));
            }
        }
    }
    let mut all = Vec::new();
    for menu in menus {
        walk(&menu.entries, &mut vec![menu.title.as_str()], &mut all);
    }
    let rank = |path: &[&str], row: &Row| {
        let label = row.label.to_lowercase();
        if label.starts_with(&query) {
            Some(0)
        } else if label.split_whitespace().any(|word| word.starts_with(&query)) {
            Some(1)
        } else if label.contains(&query) {
            Some(2)
        } else if path.join(PATH_SEPARATOR).to_lowercase().contains(&query) {
            Some(3)
        } else {
            None
        }
    };
    let mut ranked: Vec<(u8, String, &Row)> = all
        .into_iter()
        .filter_map(|(path, row)| {
            let rank = rank(&path, row)?;
            let mut shown = path.join(PATH_SEPARATOR);
            shown.push_str(PATH_SEPARATOR);
            shown.push_str(&row.label);
            Some((rank, shown, row))
        })
        .collect();
    // Stable: menu order within a rank.
    ranked.sort_by_key(|(rank, ..)| *rank);
    ranked
        .into_iter()
        .take(SEARCH_RESULTS)
        .map(|(_, shown, row)| Row { label: shown, ..row.clone() })
        .collect()
}

/// A navigation key (§3.6). Space acts as [`NavKey::Enter`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
}

/// What a navigation step did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The menus are still open (possibly a different one).
    Stay,
    /// The command was chosen; every menu is now closed.
    Run(&'static str),
    /// Every menu is now closed.
    Closed,
}

/// The open menu and its highlight. Level 0 is the top-level menu; level
/// `n + 1` is the submenu of level `n`'s highlighted row, open while
/// `highlight` is longer than `n + 1`. `depth` is the level the keyboard
/// acts on: a submenu opened by hovering leaves it on the parent level.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Nav {
    open: Option<usize>,
    highlight: Vec<Option<usize>>,
    depth: usize,
}

impl Nav {
    /// The open top-level menu.
    pub fn open_menu(&self) -> Option<usize> {
        self.open
    }

    /// The highlighted entry of each open level.
    pub fn highlight(&self) -> &[Option<usize>] {
        &self.highlight
    }

    /// The level the keyboard acts on.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Open top-level menu `menu` with nothing highlighted. The highlight is
    /// per top-level menu: opening any menu starts it afresh.
    pub fn open(&mut self, menu: usize) {
        *self = Self { open: Some(menu), highlight: vec![None], depth: 0 };
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    /// Drop whatever `menus` (rebuilt each frame) no longer has: an open
    /// menu past the end, a highlight on a row that is gone or disabled, a
    /// level whose parent row is no longer a submenu, and the levels below.
    pub fn validate(&mut self, menus: &[Menu]) {
        let Some(open) = self.open else { return };
        if open >= menus.len() {
            self.close();
            return;
        }
        for level in 0..self.highlight.len() {
            // (enabled, is a submenu) of the level's highlighted row.
            let lit = self.highlight[level]
                .and_then(|index| self.level(menus, level)?.get(index)?.row())
                .map(|r| (r.enabled, r.is_submenu()));
            if self.highlight[level].is_some() && !lit.is_some_and(|(enabled, _)| enabled) {
                self.highlight[level] = None;
            }
            if level + 1 < self.highlight.len() && lit != Some((true, true)) {
                self.highlight.truncate(level + 1);
                break;
            }
        }
        self.depth = self.depth.min(self.highlight.len() - 1);
    }

    /// Close the submenus below level `level`.
    fn close_below(&mut self, level: usize) {
        self.highlight.truncate(level + 1);
        self.depth = self.depth.min(level);
    }

    /// How many levels are showing.
    pub fn levels(&self) -> usize {
        if self.open.is_some() { self.highlight.len() } else { 0 }
    }

    /// The entries of open level `level`.
    pub fn level<'a>(&self, menus: &'a [Menu], level: usize) -> Option<&'a [Entry]> {
        let mut entries = menus.get(self.open?)?.entries.as_slice();
        for parent in 0..level {
            let row = entries.get((*self.highlight.get(parent)?)?)?.row()?;
            if !row.is_submenu() {
                return None;
            }
            entries = &row.children;
        }
        Some(entries)
    }

    /// Whether level `level`'s highlighted row has its submenu showing.
    pub fn submenu_open(&self, level: usize) -> bool {
        self.highlight.len() > level + 1
    }

    fn current<'a>(&self, menus: &'a [Menu]) -> Option<&'a Row> {
        let entries = self.level(menus, self.depth)?;
        entries.get((*self.highlight.get(self.depth)?)?)?.row()
    }

    /// Apply `key` (§3.6).
    pub fn key(&mut self, key: NavKey, menus: &[Menu]) -> Outcome {
        let Some(open) = self.open else { return Outcome::Stay };
        let count = menus.len().max(1);
        match key {
            NavKey::Down | NavKey::Up => {
                let Some(entries) = self.level(menus, self.depth) else { return Outcome::Stay };
                let from = self.highlight.get(self.depth).copied().flatten();
                let next = step(entries, from, key == NavKey::Down);
                self.highlight.truncate(self.depth + 1);
                self.highlight[self.depth] = next;
            }
            NavKey::Right => {
                if self.current(menus).is_some_and(|row| row.enabled && row.is_submenu()) {
                    self.enter(menus);
                } else {
                    self.open((open + 1) % count);
                }
            }
            NavKey::Left => {
                if self.depth > 0 {
                    self.highlight.truncate(self.depth);
                    self.depth -= 1;
                } else {
                    self.open((open + count - 1) % count);
                }
            }
            NavKey::Enter => match self.current(menus) {
                Some(row) if row.enabled && row.is_submenu() => self.enter(menus),
                Some(Row { id: Some(id), enabled: true, .. }) => {
                    let id = *id;
                    self.close();
                    return Outcome::Run(id);
                }
                _ => {}
            },
            NavKey::Escape => {
                self.close();
                return Outcome::Closed;
            }
        }
        Outcome::Stay
    }

    /// Open the highlighted row's submenu (or move into it, when hovering
    /// already opened it) and highlight its first enabled row.
    fn enter(&mut self, menus: &[Menu]) {
        self.highlight.truncate(self.depth + 1);
        self.depth += 1;
        self.highlight.push(None);
        let first = self.level(menus, self.depth).and_then(|entries| step(entries, None, true));
        self.highlight[self.depth] = first;
    }

    /// The pointer moved onto entry `index` of level `level`: highlight it
    /// (a disabled row or a separator clears the level's highlight), close
    /// deeper levels, and show the row's submenu if it has one.
    pub fn hover(&mut self, level: usize, index: usize, menus: &[Menu]) {
        if level >= self.levels() {
            return;
        }
        if self.highlight[level] == Some(index) && self.submenu_open(level) {
            self.depth = level;
            return;
        }
        let row = self.level(menus, level).and_then(|entries| entries.get(index)).and_then(Entry::row);
        self.highlight.truncate(level + 1);
        self.depth = level;
        match row {
            Some(row) if row.enabled => {
                self.highlight[level] = Some(index);
                if row.is_submenu() {
                    self.highlight.push(None);
                }
            }
            _ => self.highlight[level] = None,
        }
    }
}

/// The next (or previous) enabled row after `from`, wrapping; from nothing,
/// the first (or last).
fn step(entries: &[Entry], from: Option<usize>, forward: bool) -> Option<usize> {
    let n = entries.len();
    if n == 0 {
        return None;
    }
    let start = from.unwrap_or(if forward { n - 1 } else { 0 });
    (1..=n)
        .map(|k| if forward { (start + k) % n } else { (start + n - k) % n })
        .find(|&i| entries[i].row().is_some_and(|row| row.enabled))
}

/// The open menu as labels, for tests and for UI state over the Bus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Current {
    /// The open top-level menu's title.
    pub menu: String,
    /// The highlighted row's label at each open level.
    pub path: Vec<Option<String>>,
    /// The level the keyboard acts on.
    pub depth: usize,
}

#[derive(Clone, Default)]
struct State {
    nav: Nav,
    /// The current press began on a menu title (the opening press).
    title_press: bool,
    /// The current press began inside an open menu.
    inside_press: bool,
    /// The current press began on a level's scroll bar: its release
    /// neither runs a row nor closes the menus.
    scroll_press: bool,
    /// Each level's frame, last frame: lets the pointer travel diagonally
    /// to a submenu across other rows.
    frames: Vec<Rect>,
    current: Option<Current>,
    /// The search field's text, cleared each time its menu opens.
    query: String,
    /// The search menu just opened: give its field the keyboard once the
    /// pointer settles.
    focus_search: bool,
    /// Keyboard events held back to the next frame: a navigation key typed
    /// after text in one frame, and the keys and text after it.
    deferred: Vec<egui::Event>,
    /// The menu those keys were typed in.
    deferred_menu: Option<usize>,
}

fn state_id() -> Id {
    Id::new("toolkit.menu")
}

/// Whether a menu is open (and so has the keyboard) on `ctx`.
pub fn is_open(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<State>(state_id())).is_some_and(|s| s.nav.open.is_some())
}

/// The open menu and its highlight, as of the last frame.
pub fn current(ctx: &Context) -> Option<Current> {
    ctx.data(|d| d.get_temp::<State>(state_id())).and_then(|s| s.current)
}

const KEYS: [(Key, NavKey); 7] = [
    (Key::ArrowDown, NavKey::Down),
    (Key::ArrowUp, NavKey::Up),
    (Key::ArrowLeft, NavKey::Left),
    (Key::ArrowRight, NavKey::Right),
    (Key::Enter, NavKey::Enter),
    (Key::Space, NavKey::Enter),
    (Key::Escape, NavKey::Escape),
];

/// Draw `menus` as a menu bar into the current (left-to-right) row of `ui`
/// and any open menu above everything, and return the command chosen this
/// frame. Titles are packed edge to edge (§3.3).
pub fn bar(ui: &mut Ui, menus: &[Menu]) -> Option<&'static str> {
    bar_with(ui, menus, None)
}

/// Whether `event` edits the search field's text.
fn edits_text(event: &egui::Event) -> bool {
    match event {
        egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Cut | egui::Event::Ime(_) => true,
        egui::Event::Key { key, pressed: true, .. } => matches!(key, Key::Backspace | Key::Delete),
        _ => false,
    }
}

/// The results are built from the query before this frame's typing reaches
/// it, so a navigation key after typing in the same frame would act on the
/// old results: take it, and the keyboard events after it, out of this
/// frame to replay first in the next, when the query includes the typing.
/// Pointer and wheel events stay: egui has already applied them this frame.
fn defer_after_typing(ui: &mut Ui) -> Vec<egui::Event> {
    ui.input_mut(|i| {
        let mut typed = false;
        let cut = i.events.iter().position(|event| {
            let navigation = matches!(
                event,
                egui::Event::Key { key: Key::Enter | Key::ArrowUp | Key::ArrowDown, pressed: true, .. }
            );
            if navigation && typed {
                return true;
            }
            typed |= edits_text(event);
            false
        });
        let Some(at) = cut else { return Vec::new() };
        let keyboard = |event: &egui::Event| {
            matches!(event, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Cut | egui::Event::Copy | egui::Event::Ime(_))
        };
        let tail = i.events.split_off(at);
        let (deferred, kept): (Vec<_>, Vec<_>) = tail.into_iter().partition(keyboard);
        i.events.extend(kept);
        deferred
    })
}

/// The id of the search field (§3.5), for focus and tests.
pub fn search_field_id() -> Id {
    state_id().with("search")
}

/// [`bar`] with a search field at the top of one menu (§3.5). While the
/// field has the keyboard, typing, Space, Left and Right edit the query;
/// Up and Down move the highlight over the results; Enter runs the
/// highlighted result, else the first enabled one; Escape closes. A click
/// inside that menu never closes it.
pub fn bar_with(ui: &mut Ui, menus: &[Menu], search: Option<&Search>) -> Option<&'static str> {
    let ctx = ui.ctx().clone();
    let chrome = Chrome::of(&ctx);
    let mut st: State = ctx.data(|d| d.get_temp(state_id())).unwrap_or_default();
    // While a query is typed, the search menu lists its results instead of
    // its rows; navigation and drawing work on that view of the model.
    let searching = |st: &State| search.filter(|s| st.nav.open == Some(s.menu));
    // Keys held back last frame come first, now that the text typed before
    // them is in the query; but only into the menu and field they were
    // typed in. If the menu closed or the field lost the keyboard since,
    // they are dropped, never replayed into the application.
    let field_focused = ctx.memory(|m| m.has_focus(search_field_id()));
    let deferred = std::mem::take(&mut st.deferred);
    if !deferred.is_empty() && st.nav.open == st.deferred_menu && searching(&st).is_some() && field_focused {
        ui.input_mut(|i| {
            i.events.splice(0..0, deferred);
        });
    }
    st.deferred_menu = None;
    if searching(&st).is_some() && field_focused {
        st.deferred = defer_after_typing(ui);
        if !st.deferred.is_empty() {
            st.deferred_menu = st.nav.open;
            ctx.request_repaint();
        }
    }
    let view: Vec<Menu>;
    let menus = match searching(&st) {
        Some(s) if !st.query.trim().is_empty() => {
            let mut changed = menus.to_vec();
            changed[s.menu].entries = matching(menus, &st.query).into_iter().map(Entry::Row).collect();
            view = changed;
            view.as_slice()
        }
        _ => menus,
    };
    st.nav.validate(menus);
    let before = st.nav.clone();
    let mut fired = None;
    let typing = searching(&st).is_some() && ctx.memory(|m| m.has_focus(search_field_id()));

    // The open menu's keys come first: before focused widgets, which are
    // drawn later in the frame, and before global shortcuts (see `is_open`).
    // Keys apply in arrival order, each repeat its own step; once the menus
    // close, later keys are left for the application. The search field
    // keeps Space, Left and Right for its text.
    let mut keyed = false;
    if st.nav.open.is_some() {
        ui.input_mut(|i| {
            i.events.retain(|event| {
                let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { return true };
                if typing && matches!(key, Key::Space | Key::ArrowLeft | Key::ArrowRight) {
                    return true;
                }
                let nav = KEYS.iter().find(|(k, _)| k == key).map(|&(_, nav)| nav);
                let Some(nav) = nav.filter(|_| st.nav.open.is_some() && modifiers.matches_logically(Modifiers::NONE)) else {
                    return true;
                };
                keyed = true;
                if typing && nav == NavKey::Enter && st.nav.highlight().first().copied().flatten().is_none() {
                    // Enter with nothing highlighted runs the first enabled
                    // result (none while the query is empty).
                    let open = if st.query.trim().is_empty() { usize::MAX } else { st.nav.open.unwrap_or_default() };
                    let first = menus.get(open).and_then(|m| {
                        m.entries.iter().filter_map(Entry::row).find(|r| r.enabled && r.id.is_some()).and_then(|r| r.id)
                    });
                    if let Some(id) = first {
                        fired = Some(id);
                        st.nav.close();
                    }
                } else if let Outcome::Run(id) = st.nav.key(nav, menus) {
                    fired = Some(id);
                }
                false
            });
        });
    }

    let (pressed, released, moved, pointer) = ui.input(|i| {
        (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.delta() != Vec2::ZERO, i.pointer.interact_pos())
    });

    // Titles: open on press, hover-switch while open.
    let (palette, metrics) = (chrome.palette, chrome.metrics);
    let font = Chrome::menu_font(ui.style());
    let mut titles: Vec<(Rect, Arc<Galley>, bool)> = Vec::with_capacity(menus.len());
    let mut on_title = false;
    for (index, menu) in menus.iter().enumerate() {
        let galley = ui.painter().layout_no_wrap(menu.title.clone(), font.clone(), Color32::PLACEHOLDER);
        let size = vec2(galley.size().x + 2.0 * metrics.menu_title_padding.x, metrics.menu_title_height);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &menu.title));
        let open = st.nav.open == Some(index);
        if pressed && response.hovered() {
            on_title = true;
            st.title_press = true;
            if open {
                st.nav.close();
            } else {
                st.nav.open(index);
            }
        } else if response.clicked() && !response.clicked_by(PointerButton::Primary) {
            // Keyboard or accessibility activation toggles.
            if open { st.nav.close() } else { st.nav.open(index) }
        } else if !open && st.nav.open.is_some() && moved && response.contains_pointer() {
            // Only while moving, and only when the title is top-most, so a
            // resting pointer or an overlapping submenu never switches.
            st.nav.open(index);
        }
        titles.push((rect, galley, response.hovered()));
    }
    // The search field is cleared and takes the keyboard each time its menu
    // opens, by pointer or by key (§3.5).
    if let Some(s) = search
        && st.nav.open == Some(s.menu)
        && before.open_menu() != Some(s.menu)
    {
        st.query.clear();
        st.focus_search = true;
    }
    // Focus waits for the pointer to settle: egui hands focus back from a
    // field on the frame of any press or click that lands elsewhere, which
    // the press on the title would be.
    if st.focus_search && !ui.input(|i| i.pointer.any_pressed() || i.pointer.any_released() || i.pointer.any_down()) {
        st.focus_search = false;
        if searching(&st).is_some() {
            ctx.memory_mut(|m| m.request_focus(search_field_id()));
        }
    }
    let ppp = ctx.pixels_per_point();
    for (index, (rect, galley, hovered)) in titles.iter().enumerate() {
        if *hovered || st.nav.open == Some(index) {
            ui.painter().rect_filled(*rect, metrics.radius_sm, palette.hover);
        }
        let at = (rect.center() - galley.size() / 2.0).round_to_pixels(ppp);
        ui.painter().galley(at, galley.clone(), palette.text_dim);
    }

    // The open levels, outermost first: each level's pointer hover is
    // applied before the next level is placed, so a hover that closes a
    // submenu takes effect this frame.
    let mut frames: Vec<Rect> = Vec::new();
    let mut bars: Vec<Rect> = Vec::new();
    let mut under: Option<(usize, usize)> = None;
    if let Some(open) = st.nav.open {
        let frame = Frame::menu(ui.style());
        let margin = frame.total_margin();
        let anchor = titles[open].0;
        let bar_bottom = anchor.bottom();
        let screen = ctx.content_rect();
        // The frame hangs from the title's bottom edge (measured: its border
        // starts where the title's hover fill ends).
        let mut origin = pos2(anchor.left(), anchor.bottom());
        let bottom = screen.bottom() - metrics.edge_gap;
        // A level taller than the window below the bar scrolls.
        let tallest = (bottom - bar_bottom).max(metrics.menu_row_height + margin.sum().y);
        let mut level = 0;
        while level < st.nav.levels() {
            let Some(entries) = st.nav.level(menus, level) else { break };
            let min_width = if level == 0 { metrics.menu_min_width } else { 0.0 };
            let layout = Layout::new(ui, &chrome, entries, min_width);
            let outer = layout.size + margin.sum();
            if level == 0 {
                origin.x = origin.x.min(screen.right() - metrics.edge_gap - outer.x).max(screen.left());
            }
            let rect = Rect::from_min_size(origin, outer);
            // The search menu's field and its separator band sit above the rows.
            let header = searching(&st).filter(|_| level == 0);
            // The field, the separator and their item spacing come to about
            // one row and a separator band; the no-matches line is a row.
            let header_height = header.map_or(0.0, |_| {
                let empty = entries.is_empty() && !st.query.trim().is_empty();
                metrics.menu_row_height * if empty { 2.0 } else { 1.0 } + metrics.menu_separator_height + 6.0
            });
            let max_height =
                (bottom - origin.y).max(metrics.menu_row_height + margin.sum().y) - margin.sum().y - header_height;
            let deeper = st.frames.get(level + 1).copied().filter(|_| st.nav.submenu_open(level));
            // A key reveals the highlight of every level it acts through, so
            // a submenu entered by keyboard brings its parent row into view.
            let view = View { max_height, moved, keyed: keyed && level <= st.nav.depth(), deeper };
            let header = header.map(|s| (s, &mut st.query));
            let shown = show_level(&ctx, &chrome, level, entries, &layout, rect, &mut st.nav, menus, view, header);
            frames.push(shown.frame);
            bars.extend(shown.bar);
            if let Some(index) = shown.under {
                under = Some((level, index));
            }
            if let Some(index) = shown.activated
                && let Some(Row { id: Some(id), enabled: true, .. }) = entries[index].row()
            {
                fired = Some(*id);
                st.nav.close();
                break;
            }
            // Place the next level beside this level's highlighted row: its
            // first row level with that row, else opening upward, else
            // pressed against the window bottom, never over the bar.
            let Some(row) = st.nav.highlight().get(level).copied().flatten().and_then(|i| shown.rows.get(i).copied())
            else {
                break;
            };
            if !st.nav.submenu_open(level) {
                break;
            }
            // A parent row scrolled out of view takes its submenu with it,
            // unless a key just opened it: the row comes into view next frame.
            if !keyed && !shown.view.contains(row.center()) {
                st.nav.close_below(level);
                break;
            }
            let child = st.nav.level(menus, level + 1).map(|e| Layout::new(ui, &chrome, e, 0.0).size + margin.sum());
            let mut child = child.unwrap_or(Vec2::ZERO);
            child.y = child.y.min(tallest);
            let mut x = shown.frame.right() + metrics.submenu_gap;
            if x + child.x > screen.right() - metrics.edge_gap {
                x = (shown.frame.left() - metrics.submenu_gap - child.x).max(screen.left());
            }
            let mut y = row.top() - margin.top;
            if y + child.y > bottom {
                let upward = row.bottom() + margin.bottom - child.y;
                y = if upward >= bar_bottom { upward } else { (bottom - child.y).max(bar_bottom) };
            }
            origin = pos2(x, y.max(bar_bottom));
            level += 1;
        }
    }

    let inside = |pos: Pos2| frames.iter().any(|f| f.contains(pos));
    if pressed {
        st.inside_press = pointer.is_some_and(inside);
        st.scroll_press = pointer.is_some_and(|p| bars.iter().any(|b| b.contains(p)));
        if !on_title && !st.inside_press && st.nav.open.is_some() {
            st.nav.close();
        }
    }
    if released {
        let row = under.filter(|_| !st.scroll_press).and_then(|(level, index)| {
            st.nav.level(menus, level).and_then(|e| e.get(index)).and_then(Entry::row).map(|r| (level, index, r))
        });
        match row {
            // A scroll bar gesture only scrolls.
            _ if st.scroll_press => {}
            Some((_, _, Row { id: Some(id), enabled: true, .. })) if st.nav.open.is_some() => {
                fired = Some(*id);
                st.nav.close();
            }
            Some((level, index, row)) if row.enabled && row.is_submenu() => {
                if st.inside_press {
                    st.nav.hover(level, index, menus);
                    st.nav.depth = level;
                    st.nav.key(NavKey::Right, menus);
                }
            }
            // A click inside the search menu (on its field, say) keeps it
            // open: only a click outside closes it (§3.5).
            _ if st.inside_press && pointer.is_some_and(inside) && searching(&st).is_none() => st.nav.close(),
            _ => {}
        }
        st.title_press = false;
        st.inside_press = false;
        st.scroll_press = false;
    }

    st.current = st.nav.open.map(|m| Current {
        menu: menus[m].title.clone(),
        path: (0..st.nav.levels())
            .map(|level| {
                let index = st.nav.highlight().get(level).copied().flatten()?;
                st.nav.level(menus, level)?.get(index)?.row().map(|r| r.label.clone())
            })
            .collect(),
        depth: st.nav.depth,
    });
    st.frames = frames;
    if st.nav.open.is_none() {
        st.query.clear();
    }
    if st.nav != before || st.focus_search {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| d.insert_temp(state_id(), st));
    fired
}

/// A row's laid-out label and its right column (shortcut or arrow).
type Line = (Arc<Galley>, Option<Arc<Galley>>);

/// One level's text and size, before placement.
struct Layout {
    /// Per entry; `None` for a separator.
    lines: Vec<Option<Line>>,
    heights: Vec<f32>,
    /// The content size (inside the frame's margin).
    size: Vec2,
    /// The tick's gutter before every label, when the level has choices.
    gutter: f32,
    tick: Option<Arc<Galley>>,
}

impl Layout {
    /// `min_width` applies to the top level only: a submenu is as wide as
    /// its rows (measured, §3.5).
    fn new(ui: &Ui, chrome: &Chrome, entries: &[Entry], min_width: f32) -> Self {
        let m = &chrome.metrics;
        let font = Chrome::menu_font(ui.style());
        let painter = ui.painter();
        let galley = |text: &str| painter.layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER);
        // A level with any choice gives every label the tick's gutter, so
        // ticked and unticked labels line up (§3.5).
        let choices = entries.iter().filter_map(Entry::row).any(|r| r.checked.is_some());
        let (gutter, tick) = if choices { (galley(TICK_GUTTER).size().x, Some(galley(TICK))) } else { (0.0, None) };
        let mut width: f32 = min_width;
        let mut lines = Vec::with_capacity(entries.len());
        let mut heights = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry {
                Entry::Separator => {
                    lines.push(None);
                    heights.push(m.menu_separator_height);
                }
                Entry::Row(row) => {
                    let label = galley(&row.label);
                    let right = if row.is_submenu() { Some(galley(SUBMENU_ARROW)) } else { row.shortcut.as_deref().map(galley) };
                    // Labels never wrap: the menu widens instead (§3.4).
                    let right_width = right.as_ref().map_or(0.0, |g| m.shortcut_gap + g.size().x);
                    width = width.max(2.0 * m.menu_row_padding.x + gutter + label.size().x + right_width);
                    lines.push(Some((label, right)));
                    heights.push(m.menu_row_height);
                }
            }
        }
        let size = vec2(width, heights.iter().sum());
        Self { lines, heights, size, gutter, tick }
    }
}

struct Shown {
    frame: Rect,
    /// Each entry's rect (rows touch: zero spacing, §3.5).
    rows: Vec<Rect>,
    /// The entry under the pointer.
    under: Option<usize>,
    /// An enabled entry activated without the pointer (accessibility).
    activated: Option<usize>,
    /// Where the rows show (the scroll area's viewport).
    view: Rect,
    /// The scroll bar's strip, while the level scrolls.
    bar: Option<Rect>,
}

/// How one level is viewed this frame.
struct View {
    /// The tallest the level's content may be; taller content scrolls.
    max_height: f32,
    /// The pointer moved this frame.
    moved: bool,
    /// A key moved this level's highlight this frame: scroll it into view.
    keyed: bool,
    /// The open submenu's frame, last frame.
    deeper: Option<Rect>,
}

#[expect(clippy::too_many_arguments, reason = "one level's inputs, called from one place")]
fn show_level(
    ctx: &Context,
    chrome: &Chrome,
    level: usize,
    entries: &[Entry],
    layout: &Layout,
    rect: Rect,
    nav: &mut Nav,
    menus: &[Menu],
    view: View,
    header: Option<(&Search, &mut String)>,
) -> Shown {
    let View { max_height, moved, keyed, deeper } = view;
    let id = state_id().with(level);
    let area = Area::new(id)
        .kind(UiKind::Menu)
        .order(Order::Foreground)
        .fixed_pos(rect.min)
        .constrain(false)
        .fade_in(false)
        .show(ctx, |ui| {
            Frame::menu(ui.style())
                .show(ui, |ui| {
                    if let Some((search, query)) = header {
                        search_header(ui, chrome, search, query, layout.size.x, entries.is_empty());
                    }
                    let out = ScrollArea::vertical()
                        .id_salt(id.with("scroll"))
                        .max_height(max_height)
                        .auto_shrink([true, true])
                        .show(ui, |ui| level_rows(ui, chrome, level, entries, layout, nav, menus, moved, keyed, deeper, id));
                    let view = out.inner_rect;
                    let bar = (out.content_size.y > view.height() + 0.5).then(|| {
                        // egui's bar ends at the allocated strip's outer
                        // margin; a floating bar is wider than its strip and
                        // reaches back over the rows' right edge.
                        let s = &ui.spacing().scroll;
                        let allocated = if s.floating {
                            s.floating_allocated_width
                        } else {
                            s.bar_inner_margin + s.bar_width + s.bar_outer_margin
                        };
                        let right = view.right() + allocated;
                        let left = view.right().min(right - s.bar_outer_margin - s.bar_width);
                        Rect::from_x_y_ranges(left..=right, view.y_range())
                    });
                    (out.inner, view, bar)
                })
                .inner
        });
    let ((rows, under, activated), view, bar) = area.inner;
    Shown { frame: area.response.rect, rows, under, activated, view, bar }
}

/// The search menu's field (§3.5): egui's single-line field, 220 wide, in
/// egui's menu style (so no resting border), then egui's separator, then
/// the weak "no matches" line when a query found nothing. Measured: the
/// field spans 35 to 52.5 pt under a 32 pt bar and the rule sits at 62.
fn search_header(ui: &mut Ui, chrome: &Chrome, search: &Search, query: &mut String, width: f32, empty: bool) {
    let m = &chrome.metrics;
    ui.scope(|ui| {
        egui::containers::menu::menu_style(ui.style_mut());
        let field = TextEdit::singleline(query)
            .id(search_field_id())
            .hint_text(search.hint.as_str())
            .desired_width(width.max(SEARCH_WIDTH));
        if ui.add(field).changed() {
            ui.ctx().request_repaint();
        }
    });
    ui.separator();
    if empty && !query.trim().is_empty() {
        let (row, response) = ui.allocate_exact_size(vec2(width, m.menu_row_height), Sense::hover());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &search.empty));
        let ink = chrome.palette.text.gamma_multiply(WEAK_ALPHA);
        let galley = ui.painter().layout_no_wrap(search.empty.clone(), Chrome::menu_font(ui.style()), ink);
        let at = pos2(row.left() + m.menu_row_padding.x, row.center().y - galley.size().y / 2.0);
        ui.painter().galley(at, galley, ink);
    }
}

/// The rows of one level, inside its scroll area.
#[expect(clippy::too_many_arguments, reason = "one level's inputs, called from one place")]
fn level_rows(
    ui: &mut Ui,
    chrome: &Chrome,
    level: usize,
    entries: &[Entry],
    layout: &Layout,
    nav: &mut Nav,
    menus: &[Menu],
    moved: bool,
    keyed: bool,
    deeper: Option<Rect>,
    id: Id,
) -> (Vec<Rect>, Option<usize>, Option<usize>) {
    let (content, _) = ui.allocate_exact_size(layout.size, Sense::hover());
    let mut rows = Vec::with_capacity(entries.len());
    let mut y = content.top();
    for height in &layout.heights {
        rows.push(Rect::from_x_y_ranges(content.x_range(), y..=y + height));
        y += height;
    }
    let mut under = None;
    let mut activated = None;
    for (index, (entry, row_rect)) in entries.iter().zip(&rows).enumerate() {
        let response = ui.interact(*row_rect, id.with(index), Sense::click());
        if let Entry::Row(row) = entry {
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, row.enabled, &row.label));
            if let Some(checked) = row.checked {
                // A choice is one of a group, ticked or not: a radio menu
                // item to assistive technology and the drive layer.
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    node.set_role(egui::accesskit::Role::MenuItemRadio);
                    node.set_toggled(if checked { egui::accesskit::Toggled::True } else { egui::accesskit::Toggled::False });
                });
            }
            if row.enabled && response.clicked() && !response.clicked_by(PointerButton::Primary) {
                activated = Some(index);
            }
        }
        if response.contains_pointer() {
            under = Some(index);
        }
    }
    // A moving pointer highlights the row under it, unless it is heading
    // for the open submenu.
    let heading = deeper.is_some_and(|r| ui.input(|i| i.pointer.is_moving_towards_rect(&r)));
    if let Some(index) = under
        && moved
        && !heading
    {
        nav.hover(level, index, menus);
    }
    if keyed && let Some(row) = nav.highlight().get(level).copied().flatten().and_then(|i| rows.get(i)) {
        // At once, as a native menu does: an animated scroll would leave a
        // keyboard-opened submenu's row out of view for several frames.
        ui.scroll_to_rect_animation(*row, None, egui::style::ScrollAnimation::none());
    }
    paint(ui, chrome, level, entries, layout, &rows, nav);
    (rows, under, activated)
}

fn paint(ui: &Ui, chrome: &Chrome, level: usize, entries: &[Entry], layout: &Layout, rows: &[Rect], nav: &Nav) {
    let (p, m) = (&chrome.palette, &chrome.metrics);
    let painter = ui.painter();
    let ppp = ui.ctx().pixels_per_point();
    let highlighted = nav.highlight().get(level).copied().flatten();
    for (index, ((entry, rect), line)) in entries.iter().zip(rows).zip(&layout.lines).enumerate() {
        match (entry, line) {
            (Entry::Separator, _) => {
                let y = painter.round_to_pixel_center(rect.center().y);
                painter.hline(rect.x_range(), y, Stroke::new(1.0, p.separator));
            }
            (Entry::Row(row), Some((label, right))) => {
                let lit = highlighted == Some(index) && row.enabled;
                let parent = lit && nav.submenu_open(level);
                // A row whose submenu is open shows the neutral hover fill;
                // the highlighted row the menu highlight (§3.5).
                let (fill, ink) = if parent {
                    (Some(p.hover), p.text)
                } else if lit {
                    (Some(p.menu_highlight), p.menu_highlight_text)
                } else {
                    (None, p.text)
                };
                if let Some(fill) = fill {
                    painter.rect_filled(*rect, m.menu_highlight_radius, fill);
                }
                let ink = if row.enabled { ink } else { ink.gamma_multiply(DISABLED_ALPHA) };
                let x = rect.left() + m.menu_row_padding.x;
                if row.checked == Some(true)
                    && let Some(tick) = &layout.tick
                {
                    let at = pos2(x, rect.center().y - tick.size().y / 2.0).round_to_pixels(ppp);
                    painter.galley(at, tick.clone(), ink);
                }
                let x = x + layout.gutter;
                let at = pos2(x, rect.center().y - label.size().y / 2.0).round_to_pixels(ppp);
                painter.galley(at, label.clone(), ink);
                if let Some(right) = right {
                    // The submenu arrow keeps the label's ink; a shortcut is weak.
                    let tint = if row.is_submenu() { ink } else { ink.gamma_multiply(WEAK_ALPHA) };
                    let x = rect.right() - m.menu_row_padding.x - right.size().x;
                    let at = pos2(x, rect.center().y - right.size().y / 2.0).round_to_pixels(ppp);
                    painter.galley(at, right.clone(), tint);
                }
            }
            (Entry::Row(_), None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(id: &'static str, enabled: bool) -> Entry {
        Entry::Row(Row::command(id, id, None, enabled))
    }

    /// File: new, (disabled), open, [recent ⏵ clear], —, close.
    /// Edit: undo. Help: about.
    fn menus() -> Vec<Menu> {
        vec![
            Menu {
                title: "File".into(),
                entries: vec![
                    cmd("new", true),
                    cmd("paste", false),
                    cmd("open", true),
                    Entry::Row(Row::submenu("recent", vec![cmd("clear", true), cmd("none", false)])),
                    Entry::Separator,
                    cmd("close", true),
                ],
            },
            Menu { title: "Edit".into(), entries: vec![cmd("undo", true)] },
            Menu { title: "Help".into(), entries: vec![cmd("about", true)] },
        ]
    }

    fn opened() -> Nav {
        let mut nav = Nav::default();
        nav.open(0);
        nav
    }

    #[test]
    fn down_and_up_skip_disabled_rows_and_separators_and_wrap() {
        let m = menus();
        let mut nav = opened();
        let downs: Vec<_> = (0..5)
            .map(|_| {
                nav.key(NavKey::Down, &m);
                nav.highlight()[0]
            })
            .collect();
        assert_eq!(downs, [Some(0), Some(2), Some(3), Some(5), Some(0)]);
        let mut nav = opened();
        nav.key(NavKey::Up, &m);
        assert_eq!(nav.highlight()[0], Some(5), "Up from nothing goes to the last enabled row");
        nav.key(NavKey::Up, &m);
        assert_eq!(nav.highlight()[0], Some(3));
    }

    #[test]
    fn right_enters_a_submenu_or_opens_the_next_menu() {
        let m = menus();
        let mut nav = opened();
        nav.key(NavKey::Right, &m);
        assert_eq!(nav.open_menu(), Some(1), "nothing highlighted: next menu");
        nav.key(NavKey::Down, &m);
        nav.key(NavKey::Right, &m);
        assert_eq!(nav.open_menu(), Some(2), "command row: next menu");
        nav.key(NavKey::Right, &m);
        assert_eq!(nav.open_menu(), Some(0), "wraps from the last menu to the first");
        assert_eq!(nav.highlight(), [None], "the highlight resets per menu");
        for _ in 0..3 {
            nav.key(NavKey::Down, &m);
        }
        nav.key(NavKey::Right, &m);
        assert_eq!((nav.highlight(), nav.depth()), (&[Some(3), Some(0)][..], 1), "into the submenu, first enabled row");
        nav.key(NavKey::Down, &m);
        assert_eq!(nav.highlight()[1], Some(0), "the only enabled row stays");
    }

    #[test]
    fn left_leaves_a_submenu_or_opens_the_previous_menu() {
        let m = menus();
        let mut nav = opened();
        for _ in 0..3 {
            nav.key(NavKey::Down, &m);
        }
        nav.key(NavKey::Right, &m);
        nav.key(NavKey::Left, &m);
        assert_eq!((nav.highlight(), nav.depth()), (&[Some(3)][..], 0), "parent row stays highlighted");
        nav.key(NavKey::Left, &m);
        assert_eq!(nav.open_menu(), Some(2), "wraps to the last menu");
    }

    #[test]
    fn enter_runs_enabled_commands_only_and_escape_closes() {
        let m = menus();
        let mut nav = opened();
        assert_eq!(nav.key(NavKey::Enter, &m), Outcome::Stay, "nothing highlighted");
        nav.key(NavKey::Down, &m);
        nav.key(NavKey::Down, &m);
        assert_eq!(nav.key(NavKey::Enter, &m), Outcome::Run("open"));
        assert_eq!(nav.open_menu(), None, "running closes every menu");
        let mut nav = opened();
        nav.hover(0, 1, &m);
        assert_eq!(nav.highlight()[0], None, "a disabled row is never highlighted");
        assert_eq!(nav.key(NavKey::Enter, &m), Outcome::Stay);
        nav.hover(0, 3, &m);
        assert!(nav.submenu_open(0), "hovering a submenu row shows its submenu");
        assert_eq!(nav.depth(), 0, "the keyboard stays on the parent level");
        nav.key(NavKey::Enter, &m);
        assert_eq!((nav.depth(), nav.highlight()[1]), (1, Some(0)), "Enter moves into it like Right");
        assert_eq!(nav.key(NavKey::Escape, &m), Outcome::Closed);
        assert_eq!(nav, Nav::default());
    }

    #[test]
    fn hover_and_keys_share_one_highlight() {
        let m = menus();
        let mut nav = opened();
        nav.hover(0, 2, &m);
        nav.key(NavKey::Down, &m);
        assert_eq!(nav.highlight()[0], Some(3), "keys move on from the hovered row");
    }

    #[test]
    fn validate_drops_what_the_model_lost() {
        let mut m = menus();
        let mut nav = opened();
        nav.hover(0, 3, &m);
        nav.key(NavKey::Right, &m);
        assert_eq!((nav.highlight(), nav.depth()), (&[Some(3), Some(0)][..], 1));
        m[0].entries[3] = cmd("recent", true);
        nav.validate(&m);
        assert_eq!((nav.highlight(), nav.depth()), (&[Some(3)][..], 0), "no longer a submenu: its level closes");
        m[0].entries.truncate(2);
        nav.validate(&m);
        assert_eq!(nav.highlight(), [None], "the row is gone");
        m.clear();
        nav.validate(&m);
        assert_eq!(nav, Nav::default(), "the menu is gone");
    }

    #[test]
    fn a_level_with_choices_gives_every_label_the_tick_gutter() {
        let ctx = Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let chrome = Chrome::of(ui.ctx());
            let plain = Layout::new(ui, &chrome, &[cmd("alpha", true), cmd("beta", true)], 0.0);
            assert_eq!((plain.gutter, plain.tick.is_some()), (0.0, false), "no choices, no gutter");
            let mixed = [Entry::Row(Row::choice("alpha", "alpha", None, true, false)), cmd("beta", true)];
            let choices = Layout::new(ui, &chrome, &mixed, 0.0);
            let tick = ui.painter().layout_no_wrap(TICK_GUTTER.to_owned(), Chrome::menu_font(ui.style()), Color32::PLACEHOLDER);
            assert_eq!(choices.gutter, tick.size().x, "\"✔ \" wide, for every row of the level");
            assert!(choices.tick.is_some());
            assert_eq!(choices.size.x, plain.size.x + choices.gutter, "the level widens by the gutter");
        });
        output.textures_delta.clear();
        assert_eq!(Row::choice("a", "A", None, true, true).checked, Some(true));
        assert_eq!(Row::command("a", "A", None, true).checked, None);
    }

    #[test]
    fn separators_never_lead_trail_or_double() {
        let s = || Entry::Separator;
        let tidied = tidy(vec![s(), cmd("a", true), s(), s(), cmd("b", true), s()]);
        assert_eq!(tidied, vec![cmd("a", true), s(), cmd("b", true)]);
        assert!(!Row::submenu("x", vec![cmd("a", false)]).enabled, "no enabled child, no enabled submenu");
    }
}
