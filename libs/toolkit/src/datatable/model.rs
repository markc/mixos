// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The data table's engine: the column model, the serialisable
//! [`TableState`], the [`TableEvent`]s the widget reports, and every rule
//! that turns a click or a key into one (sort cycling, page arithmetic,
//! keyboard moves, selection clamping). Pure data: no egui, so an app's
//! engine and its tests use it without a window.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A sort direction.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    #[default]
    Ascending,
    Descending,
}

impl Order {
    /// The other direction.
    pub fn reversed(self) -> Self {
        match self {
            Order::Ascending => Order::Descending,
            Order::Descending => Order::Ascending,
        }
    }

    /// The short name agents and Bus verbs use: `asc` or `desc`.
    pub fn name(self) -> &'static str {
        match self {
            Order::Ascending => "asc",
            Order::Descending => "desc",
        }
    }

    /// The order a short name ([`Order::name`]) stands for.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "asc" => Some(Order::Ascending),
            "desc" => Some(Order::Descending),
            _ => None,
        }
    }

    /// `ordering` in this direction.
    pub fn apply(self, ordering: Ordering) -> Ordering {
        match self {
            Order::Ascending => ordering,
            Order::Descending => ordering.reverse(),
        }
    }
}

/// The sort in force: one column, one direction.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Sort {
    pub column: String,
    pub order: Order,
}

impl Sort {
    pub fn new(column: impl Into<String>, order: Order) -> Self {
        Self {
            column: column.into(),
            order,
        }
    }
}

/// How a column takes its width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Width {
    /// Always this many points.
    Fixed(f32),
    /// Starts at `initial` points; the person can drag it, down to `min`.
    Resizable { initial: f32, min: f32 },
    /// Starts at `share` (0..1) of the table's width; resizable, down to `min`.
    Share { share: f32, min: f32 },
    /// Whatever the other columns leave, at least `min`.
    Remainder { min: f32 },
}

impl Width {
    /// Whether the person can drag this column's edge.
    pub fn resizable(self) -> bool {
        matches!(self, Width::Resizable { .. } | Width::Share { .. })
    }
}

/// Where a cell's text sits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

/// One column of a table.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    /// Stable id: the sort, the widths and the Bus name the column by it.
    pub id: &'static str,
    /// Fluent key of the header label.
    pub label: &'static str,
    pub width: Width,
    pub align: Align,
    /// Whether a header click sorts by it.
    pub sortable: bool,
    /// The direction the first click sorts in (a date's newest first is
    /// `Descending`).
    pub first_order: Order,
    /// Long text is cut to the column with "…", the whole text in a tooltip.
    pub elide: bool,
}

impl Column {
    /// A start-aligned, eliding, unsortable column taking the remainder.
    pub fn new(id: &'static str, label: &'static str) -> Self {
        Self {
            id,
            label,
            width: Width::Remainder { min: 40.0 },
            align: Align::Start,
            sortable: false,
            first_order: Order::Ascending,
            elide: true,
        }
    }

    pub fn width(mut self, width: Width) -> Self {
        self.width = width;
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// Sortable, first sorted in `first`.
    pub fn sortable(mut self, first: Order) -> Self {
        self.sortable = true;
        self.first_order = first;
        self
    }

    pub fn elide(mut self, elide: bool) -> Self {
        self.elide = elide;
        self
    }
}

/// Row height: one line, at the design's row height or a little more.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    #[default]
    Compact,
    Comfortable,
}

/// How many rows a person may select.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SelectionMode {
    #[default]
    Single,
    /// Ctrl-click (Cmd on macOS) adds and removes rows.
    Multi,
}

/// How a selection event changes the selection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Pick {
    /// The row alone.
    #[default]
    Only,
    /// Add the row, or remove it if it was selected (multi only).
    Toggle,
}

/// What the table knows of the whole data set's size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Total {
    /// Exactly this many rows across every page.
    Known(u64),
    /// Not known; `more` says whether a page follows this one.
    Unknown { more: bool },
}

/// A move of the pager.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageMove {
    First,
    Previous,
    Next,
    Last,
    To(u64),
}

/// A key the table answers while it has the keyboard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableKey {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    /// Activate the focused row.
    Enter,
}

/// What the person did to the table. The table only reports: the app
/// applies each event ([`TableState::apply`] for the table's own fields)
/// and decides what a sort or a page means (a client-side reorder, or a
/// new server query).
#[derive(Clone, Debug, PartialEq)]
pub enum TableEvent {
    /// The sort is now this (`None`: unsorted, when the table allows it).
    SortChanged(Option<Sort>),
    /// Show this page (0-based).
    PageChanged(u64),
    /// The page size picker chose this size; the page goes back to 0.
    PageSizeChanged(u32),
    /// Row `index` of the page (key `key`) was selected.
    Selected {
        index: usize,
        key: String,
        pick: Pick,
    },
    /// Row `index` was activated (double-click or Enter).
    Activated { index: usize, key: String },
    /// The person dragged a column to this width (points).
    ColumnResized { column: String, width: f32 },
}

/// The page size a fresh state uses.
pub const DEFAULT_PAGE_SIZE: u32 = 50;

/// Everything a table remembers, as one serialisable struct: persist it in
/// the app's config and report it over the Bus as UI state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct TableState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<Sort>,
    /// The page shown, 0-based.
    pub page: u64,
    pub page_size: u32,
    /// The selected rows' keys, in selection order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub selection: Vec<String>,
    /// The row the keyboard is on, an index into the page's rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<usize>,
    /// Widths the person dragged, by column id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub widths: BTreeMap<String, f32>,
    pub density: Density,
}

impl Default for TableState {
    fn default() -> Self {
        Self {
            sort: None,
            page: 0,
            page_size: DEFAULT_PAGE_SIZE,
            selection: Vec::new(),
            focus: None,
            widths: BTreeMap::new(),
            density: Density::Compact,
        }
    }
}

impl TableState {
    /// A state sorted by `sort`.
    pub fn sorted(sort: Sort) -> Self {
        Self {
            sort: Some(sort),
            ..Self::default()
        }
    }

    /// The page size, never zero.
    pub fn size(&self) -> u64 {
        u64::from(self.page_size.max(1))
    }

    /// The first row of the shown page, 0-based across the whole set.
    pub fn offset(&self) -> u64 {
        self.page.saturating_mul(self.size())
    }

    /// The sort a header click on `column` asks for: another column starts
    /// at its first order; the sorted column reverses, and after its second
    /// direction goes unsorted when `allow_none`. `None` when the column
    /// does not sort.
    pub fn next_sort(
        &self,
        columns: &[Column],
        column: &str,
        allow_none: bool,
    ) -> Option<Option<Sort>> {
        let col = columns.iter().find(|c| c.id == column && c.sortable)?;
        Some(match &self.sort {
            Some(sort) if sort.column == column => {
                if allow_none && sort.order != col.first_order {
                    None
                } else {
                    Some(Sort::new(column, sort.order.reversed()))
                }
            }
            _ => Some(Sort::new(column, col.first_order)),
        })
    }

    /// The event a header click on `column` reports, if it sorts.
    pub fn header_clicked(
        &self,
        columns: &[Column],
        column: &str,
        allow_none: bool,
    ) -> Option<TableEvent> {
        self.next_sort(columns, column, allow_none)
            .map(TableEvent::SortChanged)
    }

    /// How many pages `total` makes; `None` when the total is unknown.
    /// An empty set is one (empty) page.
    pub fn pages(&self, total: Total) -> Option<u64> {
        match total {
            Total::Known(rows) => Some(rows.div_ceil(self.size()).max(1)),
            Total::Unknown { .. } => None,
        }
    }

    /// The last page, when it is known.
    pub fn last_page(&self, total: Total) -> Option<u64> {
        self.pages(total).map(|pages| pages - 1)
    }

    /// The page `to` lands on, or `None` when it goes nowhere (already
    /// there, before the first page, past the last, or Last of an unknown
    /// total).
    pub fn page_target(&self, to: PageMove, total: Total) -> Option<u64> {
        let last = self.last_page(total);
        let target = match to {
            PageMove::First => 0,
            PageMove::Previous => self.page.checked_sub(1)?,
            PageMove::Next => match total {
                Total::Unknown { more: false } => return None,
                _ => self.page + 1,
            },
            PageMove::Last => last?,
            PageMove::To(page) => page,
        };
        if last.is_some_and(|last| target > last) || target == self.page {
            return None;
        }
        Some(target)
    }

    /// Whether the pager's `to` button does anything.
    pub fn can(&self, to: PageMove, total: Total) -> bool {
        self.page_target(to, total).is_some()
    }

    /// The event `to` reports, if it moves.
    pub fn page_event(&self, to: PageMove, total: Total) -> Option<TableEvent> {
        self.page_target(to, total).map(TableEvent::PageChanged)
    }

    /// The shown rows' place in the whole set, 1-based and inclusive
    /// (`11, 20` for "11–20"), for a page of `shown` rows; `None` when the
    /// page is empty.
    pub fn range(&self, shown: usize) -> Option<(u64, u64)> {
        let shown = shown as u64;
        (shown > 0).then(|| (self.offset() + 1, self.offset() + shown))
    }

    /// Where `key` moves the focus on a page of `len` rows when `visible`
    /// rows fit in view; `None` when there is nowhere to go. With no
    /// focus, Down and Home start at the first row, Up and End at the last.
    pub fn moved(&self, key: TableKey, len: usize, visible: usize) -> Option<usize> {
        if len == 0 {
            return None;
        }
        let last = len - 1;
        let step = visible.max(1);
        let to = match (key, self.focus.map(|f| f.min(last))) {
            (TableKey::Enter, _) => return None,
            (TableKey::Home, _) => 0,
            (TableKey::End, _) => last,
            (TableKey::Down | TableKey::PageDown, None) => 0,
            (TableKey::Up | TableKey::PageUp, None) => last,
            (TableKey::Down, Some(at)) => (at + 1).min(last),
            (TableKey::Up, Some(at)) => at.saturating_sub(1),
            (TableKey::PageDown, Some(at)) => (at + step).min(last),
            (TableKey::PageUp, Some(at)) => at.saturating_sub(step),
        };
        (self.focus != Some(to)).then_some(to)
    }

    /// The events `key` reports on a page whose rows have `keys`: a move
    /// selects the row it lands on; Enter activates the focused row.
    pub fn key(&self, key: TableKey, keys: &[String], visible: usize) -> Vec<TableEvent> {
        if key == TableKey::Enter {
            return self
                .focus
                .and_then(|index| keys.get(index).map(|k| (index, k)))
                .map(|(index, key)| TableEvent::Activated {
                    index,
                    key: key.clone(),
                })
                .into_iter()
                .collect();
        }
        self.moved(key, keys.len(), visible)
            .map(|index| TableEvent::Selected {
                index,
                key: keys[index].clone(),
                pick: Pick::Only,
            })
            .into_iter()
            .collect()
    }

    /// The page's rows are now `keys` (a new page, a refresh, a removal):
    /// selected keys no longer shown are dropped, and the focus follows the
    /// first selected row, else stays where it was within the new length.
    pub fn rows_changed(&mut self, keys: &[String]) {
        self.selection.retain(|key| keys.contains(key));
        let selected = self
            .selection
            .first()
            .and_then(|key| keys.iter().position(|k| k == key));
        self.focus = match (selected, self.focus) {
            (Some(at), _) => Some(at),
            (None, _) if keys.is_empty() => None,
            (None, Some(at)) => Some(at.min(keys.len() - 1)),
            (None, None) => None,
        };
    }

    /// Whether row `key` is selected.
    pub fn is_selected(&self, key: &str) -> bool {
        self.selection.iter().any(|k| k == key)
    }

    /// Apply `event` to the table's own fields under `mode`. A new sort or
    /// page keeps the selection (the app's rows decide what is still
    /// shown, through [`TableState::rows_changed`]) and drops the focus.
    pub fn apply(&mut self, event: &TableEvent, mode: SelectionMode) {
        match event {
            TableEvent::SortChanged(sort) => {
                self.sort = sort.clone();
                self.page = 0;
                self.focus = None;
            }
            TableEvent::PageChanged(page) => {
                self.page = *page;
                self.focus = None;
            }
            TableEvent::PageSizeChanged(size) => {
                self.page_size = (*size).max(1);
                self.page = 0;
                self.focus = None;
            }
            TableEvent::Selected { index, key, pick } => {
                self.focus = Some(*index);
                match (mode, pick) {
                    (SelectionMode::Multi, Pick::Toggle) => {
                        if self.is_selected(key) {
                            self.selection.retain(|k| k != key);
                        } else {
                            self.selection.push(key.clone());
                        }
                    }
                    _ => self.selection = vec![key.clone()],
                }
            }
            TableEvent::Activated { index, key } => {
                self.focus = Some(*index);
                if !self.is_selected(key) {
                    self.selection = vec![key.clone()];
                }
            }
            TableEvent::ColumnResized { column, width } => {
                self.widths.insert(column.clone(), *width);
            }
        }
    }

    /// The table as agents read it: columns, sort, page, pages, the shown
    /// rows' range for a page of `shown` rows, selection and focus. The
    /// rows themselves are [`super::rows_json`]'s.
    pub fn describe(&self, columns: &[Column], total: Total, shown: usize) -> Value {
        let (total_value, more) = match total {
            Total::Known(rows) => (json!(rows), Value::Null),
            Total::Unknown { more } => (Value::Null, json!(more)),
        };
        json!({
            "columns": columns.iter().map(|c| json!({
                "id": c.id,
                "label": c.label,
                "sortable": c.sortable,
                "width": self.widths.get(c.id),
            })).collect::<Vec<_>>(),
            "sort": self.sort.as_ref().map(|s| &s.column),
            "order": self.sort.as_ref().map(|s| s.order.name()),
            "page": self.page,
            "page_size": self.page_size,
            "pages": self.pages(total),
            "total": total_value,
            "more": more,
            "range": self.range(shown).map(|(first, last)| [first, last]),
            "selection": self.selection,
            "focus": self.focus,
            "density": self.density,
        })
    }
}

/// Text compared the way a person reads a sorted list: case folded first,
/// then exactly, so the order is stable and total.
pub fn compare_text(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// Sort `rows` in memory by `sort`: `compare(a, b, column)` orders two
/// rows ascending by a column, and the sort's order is applied on top. A
/// stable sort, so equal rows keep their order; no sort leaves them alone.
pub fn sort_rows<T>(
    rows: &mut [T],
    sort: Option<&Sort>,
    compare: impl Fn(&T, &T, &str) -> Ordering,
) {
    if let Some(sort) = sort {
        rows.sort_by(|a, b| sort.order.apply(compare(a, b, &sort.column)));
    }
}

/// The rows of `state`'s page out of every row held in memory.
pub fn page_of<'a, T>(rows: &'a [T], state: &TableState) -> &'a [T] {
    let start = usize::try_from(state.offset())
        .unwrap_or(usize::MAX)
        .min(rows.len());
    let end = start
        .saturating_add(usize::try_from(state.size()).unwrap_or(usize::MAX))
        .min(rows.len());
    &rows[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<Column> {
        vec![
            Column::new("from", "from").sortable(Order::Ascending),
            Column::new("subject", "subject").sortable(Order::Ascending),
            Column::new("date", "date").sortable(Order::Descending),
            Column::new("size", "size"),
        ]
    }

    fn keys(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("k{i}")).collect()
    }

    #[test]
    fn a_header_click_cycles_its_sort() {
        let cols = columns();
        let mut state = TableState::default();
        // A new column starts at its first order.
        let first = state.next_sort(&cols, "date", false).unwrap();
        assert_eq!(first, Some(Sort::new("date", Order::Descending)));
        state.sort = first;
        // The same column reverses, and keeps reversing without "none".
        let second = state.next_sort(&cols, "date", false).unwrap();
        assert_eq!(second, Some(Sort::new("date", Order::Ascending)));
        state.sort = second;
        assert_eq!(
            state.next_sort(&cols, "date", false).unwrap(),
            Some(Sort::new("date", Order::Descending))
        );
        // With "none" allowed, the second direction goes unsorted.
        assert_eq!(state.next_sort(&cols, "date", true).unwrap(), None);
        // Another column restarts; an unsortable or unknown one is no click.
        assert_eq!(
            state.next_sort(&cols, "from", true).unwrap(),
            Some(Sort::new("from", Order::Ascending))
        );
        assert_eq!(state.next_sort(&cols, "size", true), None);
        assert_eq!(state.header_clicked(&cols, "nope", true), None);
    }

    #[test]
    fn a_sort_change_returns_to_the_first_page() {
        let mut state = TableState {
            page: 4,
            focus: Some(3),
            selection: vec!["k3".into()],
            ..TableState::default()
        };
        state.apply(
            &TableEvent::SortChanged(Some(Sort::new("from", Order::Ascending))),
            SelectionMode::Single,
        );
        assert_eq!((state.page, state.focus), (0, None));
        assert_eq!(state.selection, vec!["k3".to_owned()]);
    }

    #[test]
    fn page_math_known_and_unknown() {
        let mut state = TableState {
            page_size: 20,
            ..TableState::default()
        };
        assert_eq!(state.pages(Total::Known(0)), Some(1));
        assert_eq!(state.pages(Total::Known(20)), Some(1));
        assert_eq!(state.pages(Total::Known(21)), Some(2));
        assert_eq!(state.pages(Total::Unknown { more: true }), None);
        let total = Total::Known(45);
        assert_eq!(state.page_target(PageMove::Previous, total), None);
        assert_eq!(state.page_target(PageMove::First, total), None);
        assert_eq!(state.page_target(PageMove::Next, total), Some(1));
        assert_eq!(state.page_target(PageMove::Last, total), Some(2));
        assert_eq!(state.page_target(PageMove::To(9), total), None);
        state.page = 2;
        assert_eq!(state.offset(), 40);
        assert_eq!(state.range(5), Some((41, 45)));
        assert_eq!(state.range(0), None);
        assert!(!state.can(PageMove::Next, total));
        assert!(!state.can(PageMove::Last, total));
        assert_eq!(
            state.page_event(PageMove::Previous, total),
            Some(TableEvent::PageChanged(1))
        );
        // An unknown total goes on while there is more, and never to Last.
        let open = Total::Unknown { more: true };
        assert_eq!(state.page_target(PageMove::Next, open), Some(3));
        assert_eq!(state.page_target(PageMove::Last, open), None);
        assert_eq!(
            state.page_target(PageMove::Next, Total::Unknown { more: false }),
            None
        );
        // A zero page size is one row per page, never a division by zero.
        state.page_size = 0;
        assert_eq!(state.pages(Total::Known(3)), Some(3));
    }

    #[test]
    fn a_page_size_change_starts_over() {
        let mut state = TableState {
            page: 3,
            ..TableState::default()
        };
        state.apply(&TableEvent::PageSizeChanged(0), SelectionMode::Single);
        assert_eq!((state.page, state.page_size), (0, 1));
    }

    #[test]
    fn keyboard_moves_clamp_to_the_page() {
        let mut state = TableState::default();
        assert_eq!(state.moved(TableKey::Down, 0, 10), None);
        assert_eq!(state.moved(TableKey::Down, 5, 10), Some(0));
        assert_eq!(state.moved(TableKey::Up, 5, 10), Some(4));
        state.focus = Some(2);
        assert_eq!(state.moved(TableKey::Down, 5, 10), Some(3));
        assert_eq!(state.moved(TableKey::Up, 5, 10), Some(1));
        assert_eq!(state.moved(TableKey::PageDown, 5, 2), Some(4));
        assert_eq!(state.moved(TableKey::PageUp, 5, 10), Some(0));
        assert_eq!(state.moved(TableKey::Home, 5, 10), Some(0));
        assert_eq!(state.moved(TableKey::End, 5, 10), Some(4));
        state.focus = Some(4);
        assert_eq!(state.moved(TableKey::Down, 5, 10), None, "already last");
        assert_eq!(state.moved(TableKey::End, 5, 10), None);
        // A focus past a shrunken page counts from the last row.
        state.focus = Some(9);
        assert_eq!(state.moved(TableKey::Up, 5, 10), Some(3));
    }

    #[test]
    fn keys_select_and_enter_activates() {
        let mut state = TableState::default();
        let rows = keys(3);
        let events = state.key(TableKey::Down, &rows, 10);
        assert_eq!(
            events,
            vec![TableEvent::Selected {
                index: 0,
                key: "k0".into(),
                pick: Pick::Only
            }]
        );
        state.apply(&events[0], SelectionMode::Single);
        assert_eq!(state.selection, vec!["k0".to_owned()]);
        assert_eq!(
            state.key(TableKey::Enter, &rows, 10),
            vec![TableEvent::Activated {
                index: 0,
                key: "k0".into()
            }]
        );
        state.focus = None;
        assert!(state.key(TableKey::Enter, &rows, 10).is_empty());
    }

    #[test]
    fn selection_clamps_when_the_rows_change() {
        let mut state = TableState {
            selection: vec!["k7".into()],
            focus: Some(7),
            ..TableState::default()
        };
        // The selected row moved: the focus follows it.
        let mut moved = keys(10);
        moved.swap(2, 7);
        state.rows_changed(&moved);
        assert_eq!(state.focus, Some(2));
        // The selected row is gone: dropped, focus kept within the page.
        state.focus = Some(8);
        state.rows_changed(&keys(5));
        assert!(state.selection.is_empty());
        assert_eq!(state.focus, Some(4));
        state.rows_changed(&[]);
        assert_eq!(state.focus, None);
    }

    #[test]
    fn multi_selection_toggles_single_replaces() {
        let mut state = TableState::default();
        let pick = |index: usize, pick| TableEvent::Selected {
            index,
            key: format!("k{index}"),
            pick,
        };
        state.apply(&pick(1, Pick::Only), SelectionMode::Multi);
        state.apply(&pick(3, Pick::Toggle), SelectionMode::Multi);
        assert_eq!(state.selection, vec!["k1".to_owned(), "k3".to_owned()]);
        state.apply(&pick(1, Pick::Toggle), SelectionMode::Multi);
        assert_eq!(state.selection, vec!["k3".to_owned()]);
        // Single mode ignores the toggle.
        state.apply(&pick(2, Pick::Toggle), SelectionMode::Single);
        assert_eq!(state.selection, vec!["k2".to_owned()]);
        assert_eq!(state.focus, Some(2));
    }

    #[test]
    fn resizes_are_remembered_and_the_state_round_trips() {
        let mut state = TableState::sorted(Sort::new("date", Order::Descending));
        state.apply(
            &TableEvent::ColumnResized {
                column: "from".into(),
                width: 180.0,
            },
            SelectionMode::Single,
        );
        let text = serde_json::to_string(&state).unwrap();
        let back: TableState = serde_json::from_str(&text).unwrap();
        assert_eq!(back, state);
        assert_eq!(back.widths.get("from"), Some(&180.0));
        // A partial record keeps the defaults.
        let partial: TableState = serde_json::from_str(r#"{"page": 2}"#).unwrap();
        assert_eq!(partial.page_size, DEFAULT_PAGE_SIZE);
        assert_eq!(partial.page, 2);
    }

    #[test]
    fn client_side_sort_and_paging() {
        let mut rows = vec![("b", 2), ("A", 3), ("a", 1), ("c", 2)];
        let compare = |x: &(&str, i32), y: &(&str, i32), column: &str| match column {
            "name" => compare_text(x.0, y.0),
            _ => x.1.cmp(&y.1),
        };
        sort_rows(
            &mut rows,
            Some(&Sort::new("name", Order::Ascending)),
            compare,
        );
        assert_eq!(rows, vec![("A", 3), ("a", 1), ("b", 2), ("c", 2)]);
        sort_rows(&mut rows, Some(&Sort::new("n", Order::Descending)), compare);
        // Stable: the two 2s keep their order.
        assert_eq!(rows, vec![("A", 3), ("b", 2), ("c", 2), ("a", 1)]);
        let before = rows.clone();
        sort_rows(&mut rows, None, compare);
        assert_eq!(rows, before);
        let state = TableState {
            page: 1,
            page_size: 3,
            ..TableState::default()
        };
        assert_eq!(page_of(&rows, &state), &[("a", 1)]);
        let past = TableState { page: 9, ..state };
        assert!(page_of(&rows, &past).is_empty());
    }

    #[test]
    fn the_description_names_sort_page_and_range() {
        let state = TableState {
            sort: Some(Sort::new("date", Order::Descending)),
            page: 1,
            page_size: 10,
            selection: vec!["k12".into()],
            focus: Some(2),
            ..TableState::default()
        };
        let d = state.describe(&columns(), Total::Known(25), 10);
        assert_eq!(d["sort"], "date");
        assert_eq!(d["order"], "desc");
        assert_eq!(d["pages"], 3);
        assert_eq!(d["range"], json!([11, 20]));
        assert_eq!(d["selection"], json!(["k12"]));
        assert_eq!(d["columns"][2]["sortable"], true);
        let open = state.describe(&columns(), Total::Unknown { more: true }, 0);
        assert!(open["pages"].is_null() && open["total"].is_null());
        assert_eq!(open["more"], true);
    }
}
