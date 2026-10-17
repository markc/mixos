// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Data tables: a header of sortable, resizable columns over virtualised
//! one-line rows, with a pager footer, keyboard navigation and a drive
//! surface for agents. Built on `egui_extras`' table for layout, scrolling
//! and column resizing.
//!
//! The table is data-agnostic and engine first ([`model`]):
//!
//! - the app describes its [`Column`]s once (id, Fluent label key, width
//!   policy, alignment, sortability, eliding);
//! - its rows come through the [`Rows`] trait (or [`TextRows`]), one
//!   [`Cell`] per column and an optional [`RowStyle`] per row (strong for
//!   unread mail), with [`Rows::custom`] for cells that draw widgets;
//! - everything the table remembers is one serialisable [`TableState`]
//!   (sort, page, page size, selection, focus, dragged widths, density),
//!   which the app owns, persists in its config and reports over the Bus;
//! - [`DataTable::show`] only *reports* what the person did, as
//!   [`TableEvent`]s. The app applies them ([`TableState::apply`] for the
//!   table's own fields) and decides whether a sort or a page is answered
//!   in memory ([`sort_rows`], [`page_of`]) or by a server query.
//!
//! Interaction: a click selects a row (Ctrl-click toggles one in
//! [`SelectionMode::Multi`]), a double-click or Enter activates it, and once
//! the table has the keyboard Up, Down, Page Up, Page Down, Home and End
//! move the selection, scrolling it into view. A header click cycles its
//! column's sort; dragging a header edge resizes a resizable column.
//!
//! Agents: the table is an AccessKit `Table` node labelled with its target
//! id, each row a `Row` node labelled with its cells' text (comma
//! separated) and marked selected, each header a button carrying its sort
//! direction, and the pager's buttons are labelled ("Next page", …). The
//! drive verbs (`<app>.ui.tree role=Row`, `<app>.ui.click`) read and drive
//! it as a hand would. For the data itself an app reports
//! [`TableState::describe`] and [`rows_json`] in its own state verb.
//!
//! Colours, row heights and type all come from the theme: the row height
//! is the chrome's menu-row height (a design token), compact by default.

pub mod model;
pub mod verbs;

pub use model::{
    Align, Column, DEFAULT_PAGE_SIZE, Density, Order, PageMove, Pick, SelectionMode, Sort,
    TableEvent, TableKey, TableState, Total, Width, compare_text, page_of, sort_rows,
};

use egui::accesskit::{Role, SortDirection};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Color32, EventFilter, FontId, Id, Key, Layout, Modifiers, Rect, Sense, Stroke, StrokeKind,
    TextStyle, Ui, UiBuilder, WidgetInfo, WidgetType, pos2, vec2,
};
use egui_extras::TableBuilder;
use serde_json::{Value, json};

use crate::button::{IconButton, paint_icon};
use crate::chrome::Chrome;
use crate::icons::Icon;
use crate::strings::Strings;

/// One cell: its text, and icons drawn at the cell's end (markers such as a
/// flag or an attachment). The text is what tooltips, row labels and
/// [`rows_json`] carry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    pub text: String,
    pub icons: Vec<Icon>,
    /// Drawn in the weak text colour.
    pub weak: bool,
}

impl Cell {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// Add a marker icon at the cell's end.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icons.push(icon);
        self
    }

    pub fn weak(mut self, weak: bool) -> Self {
        self.weak = weak;
        self
    }
}

/// A row's look.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RowStyle {
    /// Emphasised: the medium face in the strong text colour.
    pub strong: bool,
    /// Every cell in the weak text colour.
    pub weak: bool,
}

/// The rows of the shown page.
pub trait Rows {
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Row `row`'s stable key (the selection holds keys).
    fn key(&self, row: usize) -> String;

    /// Row `row`'s cell in column `column` (a [`Column::id`]).
    fn cell(&self, row: usize, column: &str) -> Cell;

    fn style(&self, _row: usize) -> RowStyle {
        RowStyle::default()
    }

    /// Draw the cell yourself; `true` when drawn, so the table skips its
    /// text. The cell's [`Rows::cell`] text still labels the row.
    fn custom(&self, _ui: &mut Ui, _row: usize, _column: &str) -> bool {
        false
    }
}

/// One row of [`TextRows`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextRow {
    pub key: String,
    /// One cell per column, in [`TextRows::columns`] order.
    pub cells: Vec<Cell>,
    pub style: RowStyle,
}

/// Rows held as cells: the simple [`Rows`] for tests and small tables.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextRows {
    /// The column ids, in the cells' order.
    pub columns: Vec<&'static str>,
    pub rows: Vec<TextRow>,
}

impl Rows for TextRows {
    fn len(&self) -> usize {
        self.rows.len()
    }

    fn key(&self, row: usize) -> String {
        self.rows[row].key.clone()
    }

    fn cell(&self, row: usize, column: &str) -> Cell {
        self.columns
            .iter()
            .position(|c| *c == column)
            .and_then(|i| self.rows[row].cells.get(i).cloned())
            .unwrap_or_default()
    }

    fn style(&self, row: usize) -> RowStyle {
        self.rows[row].style
    }
}

/// The shown rows as agents read them: each row's key, its cells' text by
/// column id, and whether it is strong.
pub fn rows_json(columns: &[Column], rows: &dyn Rows) -> Value {
    (0..rows.len())
        .map(|row| {
            let cells: serde_json::Map<String, Value> = columns
                .iter()
                .map(|c| (c.id.to_owned(), json!(rows.cell(row, c.id).text)))
                .collect();
            json!({"key": rows.key(row), "cells": cells, "strong": rows.style(row).strong})
        })
        .collect()
}

/// The height of one row at `density`: the chrome's menu-row height
/// (compact), or that plus an item gap above and below (comfortable).
pub fn row_height(ui: &Ui, density: Density) -> f32 {
    let base = Chrome::of(ui.ctx()).metrics.menu_row_height;
    match density {
        Density::Compact => base,
        Density::Comfortable => base + 2.0 * ui.spacing().item_spacing.y,
    }
}

/// The keyboard focus target of the table with target id `id`.
pub fn focus_id(id: &str) -> Id {
    Id::new(("toolkit.datatable", id))
}

/// Whether the table with target id `id` has the keyboard: what an app's
/// focus-scoped shortcuts (single keys such as Delete) test.
pub fn is_focused(ctx: &egui::Context, id: &str) -> bool {
    ctx.memory(|m| m.has_focus(focus_id(id)))
}

/// Give the table with target id `id` the keyboard.
pub fn focus(ctx: &egui::Context, id: &str) {
    ctx.memory_mut(|m| m.request_focus(focus_id(id)));
}

/// The drag-and-drop payload of a row drag (egui's [`egui::DragAndDrop`]):
/// the table it left and its rows' keys. A drop target reads it with
/// `DragAndDrop::payload::<RowsDrag>(ctx)` and checks `table`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowsDrag {
    pub table: String,
    pub keys: Vec<String>,
}

/// The keys a focused table answers, without modifiers.
const KEYS: [(Key, TableKey); 7] = [
    (Key::ArrowUp, TableKey::Up),
    (Key::ArrowDown, TableKey::Down),
    (Key::PageUp, TableKey::PageUp),
    (Key::PageDown, TableKey::PageDown),
    (Key::Home, TableKey::Home),
    (Key::End, TableKey::End),
    (Key::Enter, TableKey::Enter),
];

/// One pass's layout inputs to the table body.
#[derive(Clone, Copy)]
struct Pass<'p> {
    /// The shown columns (the hidden ones left out).
    columns: &'p [Column],
    keys: &'p [String],
    height: f32,
    body: f32,
    focused: bool,
    scroll_to: Option<usize>,
}

/// A table over one page of rows. Build it each frame from the app's
/// state and call [`DataTable::show`].
pub struct DataTable<'a> {
    id: &'a str,
    columns: &'a [Column],
    state: &'a TableState,
    total: Total,
    strings: Option<&'a Strings>,
    allow_unsorted: bool,
    selection: SelectionMode,
    page_sizes: &'a [u32],
    pager: bool,
    striped: bool,
    empty: Option<String>,
    context: Option<ContextMenu<'a>>,
    drag: Option<&'a dyn Fn(usize) -> String>,
    everything: Option<Everything>,
    /// The selection's size when it is more than the page's rows.
    selected: Option<u64>,
}

/// The footer's "select every row" offer (multi-selection tables).
#[derive(Clone, Debug, PartialEq)]
pub struct Everything {
    /// The link's words ("Select all 2,866 in folder").
    pub link: String,
    /// Shown in its place once every row is selected ("All 2,866 selected").
    pub done: Option<String>,
}

/// Draws a row's context menu: given the menu's `Ui` and the row's index.
pub type ContextMenu<'a> = &'a mut dyn FnMut(&mut Ui, usize);

impl<'a> DataTable<'a> {
    /// The table with target id `id` (its egui id, focus target and
    /// accessible label) over `columns`, showing `state`.
    pub fn new(id: &'a str, columns: &'a [Column], state: &'a TableState) -> Self {
        Self {
            id,
            columns,
            state,
            total: Total::Unknown { more: false },
            strings: None,
            allow_unsorted: false,
            selection: SelectionMode::Single,
            page_sizes: &[],
            pager: true,
            striped: false,
            empty: None,
            context: None,
            drag: None,
            everything: None,
            selected: None,
        }
    }

    /// Offer, in the footer, to select every row of the whole set once the
    /// page is all selected (multi-selection).
    pub fn select_everything(mut self, everything: Everything) -> Self {
        self.everything = Some(everything);
        self
    }

    /// The selection is `count` rows, more than the page holds (every row
    /// of the set is selected): the footer counts those.
    pub fn selected_count(mut self, count: Option<u64>) -> Self {
        self.selected = count;
        self
    }

    /// A right-click on a row reports [`TableEvent::ContextMenu`] (after
    /// selecting the row, when it was not) and opens a popup that `menu`
    /// fills, for that row.
    pub fn context_menu(mut self, menu: ContextMenu<'a>) -> Self {
        self.context = Some(menu);
        self
    }

    /// Rows can be dragged: a drag carries the selection (or the one
    /// unselected row it starts on) as a [`RowsDrag`] payload, reports
    /// [`TableEvent::DragStarted`], and shows `label(count)` ("3 messages")
    /// by the pointer.
    pub fn draggable(mut self, label: &'a dyn Fn(usize) -> String) -> Self {
        self.drag = Some(label);
        self
    }

    /// What is known of the whole set's size (the pager reads it).
    pub fn total(mut self, total: Total) -> Self {
        self.total = total;
        self
    }

    /// The app's catalogue, for the column labels (else the toolkit's).
    pub fn strings(mut self, strings: &'a Strings) -> Self {
        self.strings = Some(strings);
        self
    }

    /// A third header click returns to unsorted.
    pub fn allow_unsorted(mut self, allow: bool) -> Self {
        self.allow_unsorted = allow;
        self
    }

    pub fn selection(mut self, mode: SelectionMode) -> Self {
        self.selection = mode;
        self
    }

    /// Offer these page sizes in the footer (none: no picker).
    pub fn page_sizes(mut self, sizes: &'a [u32]) -> Self {
        self.page_sizes = sizes;
        self
    }

    /// Show the pager footer (the default).
    pub fn pager(mut self, pager: bool) -> Self {
        self.pager = pager;
        self
    }

    /// Stripe alternate rows in the card colour.
    pub fn striped(mut self, striped: bool) -> Self {
        self.striped = striped;
        self
    }

    /// What an empty page says.
    pub fn empty_text(mut self, text: impl Into<String>) -> Self {
        self.empty = Some(text.into());
        self
    }

    fn label(&self, key: &str) -> String {
        match self.strings {
            Some(strings) => crate::command::label_text(strings, key),
            None => crate::strings::own(key),
        }
    }

    /// Draw the table into the space `ui` has left, the pager along its
    /// bottom, and report what the person did.
    pub fn show(mut self, ui: &mut Ui, rows: &dyn Rows) -> Vec<TableEvent> {
        let mut events = Vec::new();
        let mut context = self.context.take();
        let columns = self.state.visible(self.columns);
        let builder = UiBuilder::new().id_salt(("toolkit.datatable", self.id));
        ui.scope_builder(builder, |ui| {
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(Role::Table);
                node.set_label(self.id);
                node.set_row_count(rows.len());
                node.set_column_count(columns.len());
            });
            let height = row_height(ui, self.state.density);
            let keys: Vec<String> = (0..rows.len()).map(|row| rows.key(row)).collect();
            let focus = focus_id(self.id);
            let focused = ui.memory(|m| m.has_focus(focus));
            // The table takes the keyboard from under its rows (registered
            // first, so a row's click wins and hands the focus here).
            let back = ui.interact(ui.available_rect_before_wrap(), focus, Sense::click());
            if back.clicked() {
                back.request_focus();
            }
            let footer = if self.pager { height } else { 0.0 };
            let body = (ui.available_height() - footer - height).max(height);
            let visible = (body / height).floor().max(1.0) as usize;
            let mut scroll_to = None;
            if focused {
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        focus,
                        EventFilter {
                            vertical_arrows: true,
                            ..EventFilter::default()
                        },
                    )
                });
                let multi = self.selection == SelectionMode::Multi;
                for (key, press) in KEYS {
                    if ui.input_mut(|i| i.consume_key(Modifiers::NONE, key)) {
                        for event in self.state.key(press, &keys, visible) {
                            if let TableEvent::Selected { index, .. } = &event {
                                scroll_to = Some(*index);
                            }
                            events.push(event);
                        }
                    }
                    // Shift with a moving key extends the selection.
                    if multi
                        && press != TableKey::Enter
                        && ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, key))
                        && let Some(event) = self.state.extend(press, &keys, visible)
                    {
                        if let TableEvent::SelectionChanged { focus, .. } = &event {
                            scroll_to = *focus;
                        }
                        events.push(event);
                    }
                }
                if multi {
                    // Ctrl+A only while the table has the keyboard: in a
                    // text field it still selects the text.
                    if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
                        events.extend(self.state.select_all(&keys));
                    }
                    if let Some(event) = self.state.collapse(&keys)
                        && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
                    {
                        events.push(event);
                    }
                }
            }
            let layout = Pass {
                columns: &columns,
                keys: &keys,
                height,
                body,
                focused,
                scroll_to,
            };
            self.table(ui, rows, &layout, &mut context, &mut events);
            self.drag_preview(ui);
            if self.pager {
                self.footer(ui, rows.len(), height, &mut events);
            }
        });
        events
    }

    fn table(
        &self,
        ui: &mut Ui,
        rows: &dyn Rows,
        pass: &Pass,
        context: &mut Option<ContextMenu<'a>>,
        events: &mut Vec<TableEvent>,
    ) {
        let Pass {
            columns,
            keys,
            height,
            body,
            focused,
            scroll_to,
        } = *pass;
        let pad = ui.spacing().item_spacing.x / 2.0;
        // Rows abut: the selection and hover fills meet.
        ui.spacing_mut().item_spacing.y = 0.0;
        // The rules are the header's own; a column edge shows only while
        // its resize handle is hovered or dragged.
        let rule = ui.visuals().widgets.noninteractive.bg_stroke;
        ui.visuals_mut().widgets.noninteractive.bg_stroke.color = Color32::TRANSPARENT;
        let last = columns.len().saturating_sub(1);
        let available = ui.available_width();
        let sense = if self.drag.is_some() {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        // The column set is part of the id: hiding one starts its widths over.
        let shape: Vec<&str> = columns.iter().map(|c| c.id).collect();
        let mut builder = TableBuilder::new(ui)
            .id_salt(("toolkit.datatable.columns", self.id, shape))
            .striped(self.striped)
            .resizable(true)
            .sense(sense)
            .cell_layout(Layout::left_to_right(egui::Align::Center))
            .auto_shrink(false)
            .min_scrolled_height(body)
            .max_scroll_height(body);
        for column in columns {
            builder = builder.column(egui_column(column, self.state, available));
        }
        if let Some(row) = scroll_to {
            builder = builder.scroll_to_row(row, None);
        }
        let mut widths = Vec::new();
        let focus = focus_id(self.id);
        let output = builder
            .header(height, |mut header| {
                for (n, column) in columns.iter().enumerate() {
                    let sorted = self
                        .state
                        .sort
                        .as_ref()
                        .filter(|s| s.column == column.id)
                        .map(|s| s.order);
                    let label = self.label(column.label);
                    let mut whole = Rect::NOTHING;
                    let (_, response) = header.col(|ui| {
                        whole = ui.max_rect();
                        header_cell(ui, column, &label, sorted, pad, rule, n < last);
                    });
                    response.widget_info(|| {
                        WidgetInfo::labeled(WidgetType::Button, column.sortable, &label)
                    });
                    header_node(&response, whole, sorted);
                    if column.sortable && response.clicked() {
                        events.extend(self.state.header_clicked(
                            self.columns,
                            column.id,
                            self.allow_unsorted,
                        ));
                    }
                }
            })
            .body(|table| {
                widths = table.widths().to_vec();
                table.rows(height, rows.len(), |mut row| {
                    let index = row.index();
                    let key = &keys[index];
                    let selected = self.state.is_selected(key);
                    row.set_selected(selected);
                    let style = rows.style(index);
                    let mut texts = Vec::new();
                    let mut whole = Rect::NOTHING;
                    for column in columns {
                        let cell = rows.cell(index, column.id);
                        let mut elided = false;
                        let (_, response) = row.col(|ui| {
                            whole = whole.union(ui.max_rect());
                            if !rows.custom(ui, index, column.id) {
                                elided = paint_cell(ui, &cell, column, style, selected, pad);
                            }
                        });
                        if elided {
                            response.on_hover_text(cell.text.clone());
                        }
                        if !cell.text.is_empty() {
                            texts.push(cell.text);
                        }
                    }
                    let response = row.response();
                    let label = texts.join(", ");
                    response.widget_info(|| {
                        WidgetInfo::selected(WidgetType::Button, true, selected, &label)
                    });
                    // The whole row, this pass: the cells' own responses
                    // carry the last pass's content rects.
                    response.ctx.accesskit_node_builder(response.id, |node| {
                        node.set_role(Role::Row);
                        node.set_bounds(bounds(whole));
                        // A row is selected, not toggled.
                        node.clear_toggled();
                        node.set_selected(selected);
                    });
                    if response.double_clicked() {
                        events.push(TableEvent::Activated {
                            index,
                            key: key.clone(),
                        });
                    } else if response.clicked() {
                        let multi = self.selection == SelectionMode::Multi;
                        let modifiers = response.ctx.input(|i| i.modifiers);
                        if multi && modifiers.shift {
                            events.extend(self.state.range_to(index, keys));
                        } else {
                            let toggle = multi && modifiers.command;
                            events.push(TableEvent::Selected {
                                index,
                                key: key.clone(),
                                pick: if toggle { Pick::Toggle } else { Pick::Only },
                            });
                        }
                    }
                    if response.secondary_clicked() {
                        // A row outside the selection is selected first.
                        if !selected {
                            events.push(TableEvent::Selected {
                                index,
                                key: key.clone(),
                                pick: Pick::Only,
                            });
                        }
                        events.push(TableEvent::ContextMenu {
                            index,
                            key: key.clone(),
                        });
                    }
                    if let Some(menu) = context.as_mut() {
                        response.context_menu(|ui| (*menu)(ui, index));
                    }
                    if self.drag.is_some() && response.drag_started() {
                        let keys = self.state.drag_keys(key, keys);
                        if !selected {
                            events.push(TableEvent::Selected {
                                index,
                                key: key.clone(),
                                pick: Pick::Only,
                            });
                        }
                        egui::DragAndDrop::set_payload(
                            &response.ctx,
                            RowsDrag {
                                table: self.id.to_owned(),
                                keys: keys.clone(),
                            },
                        );
                        events.push(TableEvent::DragStarted { keys });
                    }
                    if response.clicked()
                        || response.double_clicked()
                        || response.secondary_clicked()
                    {
                        response.ctx.memory_mut(|m| m.request_focus(focus));
                    }
                    // The keyboard's row, where it is not the selection.
                    if focused && self.state.focus == Some(index) && !selected {
                        let visuals = &response.ctx.global_style().visuals;
                        response.ctx.layer_painter(response.layer_id).rect_stroke(
                            whole,
                            visuals.widgets.inactive.corner_radius,
                            visuals.selection.stroke,
                            StrokeKind::Inside,
                        );
                    }
                });
            });
        if rows.is_empty()
            && let Some(text) = &self.empty
        {
            let weak = ui.visuals().weak_text_color();
            let font = TextStyle::Body.resolve(ui.style());
            let top = output.inner_rect.center_top() + vec2(0.0, height);
            ui.painter()
                .text(top, egui::Align2::CENTER_TOP, text, font, weak);
        }
        self.resized(ui, columns, &widths, events);
    }

    /// The count badge by the pointer while this table's rows are dragged.
    fn drag_preview(&self, ui: &Ui) {
        let Some(label) = self.drag else {
            return;
        };
        let Some(drag) = egui::DragAndDrop::payload::<RowsDrag>(ui.ctx()) else {
            return;
        };
        let Some(pointer) = ui.ctx().pointer_latest_pos() else {
            return;
        };
        if drag.table != self.id {
            return;
        }
        let chrome = Chrome::of(ui.ctx());
        let p = &chrome.palette;
        let font = TextStyle::Body.resolve(ui.style());
        let layer = egui::LayerId::new(egui::Order::Tooltip, focus_id(self.id).with("drag"));
        let painter = ui.ctx().layer_painter(layer);
        let galley = painter.layout_no_wrap(label(drag.keys.len()), font, p.primary_text);
        let pad = ui.spacing().button_padding;
        let rect = Rect::from_min_size(pointer + vec2(12.0, 12.0), galley.size() + 2.0 * pad);
        painter.rect_filled(rect, f32::from(chrome.metrics.radius), p.primary_bg);
        painter.galley(rect.min + pad, galley, p.primary_text);
    }

    /// Report a resizable column whose width changed since the last frame,
    /// once the drag that changed it ends.
    fn resized(&self, ui: &Ui, columns: &[Column], widths: &[f32], events: &mut Vec<TableEvent>) {
        let focus = focus_id(self.id);
        let (seen, waiting) = (focus.with("widths"), focus.with("resized"));
        // By column id, so showing or hiding a column moves nothing.
        let now: Vec<(&'static str, f32)> = columns
            .iter()
            .map(|c| c.id)
            .zip(widths.iter().copied())
            .collect();
        let last: Vec<(&'static str, f32)> = ui.data(|d| d.get_temp(seen)).unwrap_or_default();
        let mut pending: Vec<&'static str> = ui.data(|d| d.get_temp(waiting)).unwrap_or_default();
        for (column, (id, width)) in columns.iter().zip(&now) {
            let moved = last
                .iter()
                .find(|(seen, _)| seen == id)
                .is_some_and(|(_, before)| (width - before).abs() > 0.5);
            if column.width.resizable() && moved && !pending.contains(id) {
                pending.push(id);
            }
        }
        if !ui.input(|i| i.pointer.any_down()) {
            for id in pending.drain(..) {
                if let Some((_, width)) = now.iter().find(|(c, _)| *c == id) {
                    events.push(TableEvent::ColumnResized {
                        column: id.to_owned(),
                        width: width.round(),
                    });
                }
            }
        }
        ui.data_mut(|d| {
            d.insert_temp(seen, now);
            d.insert_temp(waiting, pending);
        });
    }

    /// The pager: the shown range on the left; the page size picker and
    /// first, previous, next and last on the right.
    /// After the range: "N selected" while several rows are, and, with the
    /// whole page selected and more rows than it holds, the link that
    /// selects every row ([`TableEvent::SelectEverything`]), or the app's
    /// word that it has.
    fn selection_note(&self, ui: &mut Ui, shown: usize, events: &mut Vec<TableEvent>) {
        let count = self.selected.unwrap_or(self.state.selection.len() as u64);
        if count < 2 {
            return;
        }
        let note = crate::strings::own_with("datatable-selected", &[("count", &count.to_string())]);
        ui.label(egui::RichText::new(note).strong());
        let Some(everything) = &self.everything else {
            return;
        };
        let whole_page = shown > 0 && self.state.selection.len() >= shown;
        let more = matches!(self.total, Total::Known(all) if all > shown as u64);
        match &everything.done {
            Some(done) => {
                ui.label(egui::RichText::new(done).weak());
            }
            None if whole_page && more => {
                let link = crate::button::Link::new(Icon::Check, &everything.link);
                if ui.add(link).clicked() {
                    events.push(TableEvent::SelectEverything);
                }
            }
            None => {}
        }
    }

    fn footer(&self, ui: &mut Ui, shown: usize, height: f32, events: &mut Vec<TableEvent>) {
        let state = self.state;
        let total = self.total;
        let range = match (state.range(shown), total) {
            (Some((first, last)), Total::Known(all)) => crate::strings::own_with(
                "datatable-range",
                &[
                    ("first", &first.to_string()),
                    ("last", &last.to_string()),
                    ("total", &all.to_string()),
                ],
            ),
            (Some((first, last)), Total::Unknown { .. }) => crate::strings::own_with(
                "datatable-range-open",
                &[("first", &first.to_string()), ("last", &last.to_string())],
            ),
            (None, _) => crate::strings::own("datatable-none"),
        };
        let size = vec2(ui.available_width(), height);
        ui.allocate_ui_with_layout(size, Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = ui.spacing().item_spacing.x / 2.0;
            egui::Sides::new().height(height).show(
                ui,
                |ui| {
                    ui.add_space(ui.spacing().item_spacing.x);
                    ui.label(egui::RichText::new(range).weak());
                    self.selection_note(ui, shown, events);
                },
                |ui| {
                    // Right to left: Last is added first.
                    for (to, icon, key) in [
                        (PageMove::Last, Icon::ChevronsRight, "datatable-last-page"),
                        (PageMove::Next, Icon::ChevronRight, "datatable-next-page"),
                        (
                            PageMove::Previous,
                            Icon::ChevronLeft,
                            "datatable-previous-page",
                        ),
                        (PageMove::First, Icon::ChevronsLeft, "datatable-first-page"),
                    ] {
                        let button = IconButton::new(icon)
                            .size(height)
                            .tooltip(crate::strings::own(key));
                        if ui.add_enabled(state.can(to, total), button).clicked() {
                            events.extend(state.page_event(to, total));
                        }
                    }
                    if !self.page_sizes.is_empty() {
                        let options: Vec<String> =
                            self.page_sizes.iter().map(u32::to_string).collect();
                        let mut chosen = self
                            .page_sizes
                            .iter()
                            .position(|s| *s == state.page_size)
                            .unwrap_or(0);
                        let picker = crate::combo::show(
                            ui,
                            ("toolkit.datatable.page-size", self.id),
                            &mut chosen,
                            &options,
                            Some(height * 2.5),
                        );
                        if picker.changed()
                            && let Some(size) = self.page_sizes.get(chosen)
                        {
                            events.push(TableEvent::PageSizeChanged(*size));
                        }
                        ui.label(
                            egui::RichText::new(crate::strings::own("datatable-page-size")).weak(),
                        );
                    }
                },
            );
        });
    }
}

/// The egui column for `column`, starting at the width the person last
/// dragged it to.
fn egui_column(column: &Column, state: &TableState, available: f32) -> egui_extras::Column {
    let dragged = state.widths.get(column.id).copied();
    let out = match column.width {
        Width::Fixed(width) => egui_extras::Column::exact(width).resizable(false),
        Width::Resizable { initial, min } => {
            egui_extras::Column::initial(dragged.unwrap_or(initial).max(min))
                .at_least(min)
                .resizable(true)
        }
        Width::Share { share, min } => {
            egui_extras::Column::initial(dragged.unwrap_or(available * share).max(min))
                .at_least(min)
                .resizable(true)
        }
        Width::Remainder { min } => egui_extras::Column::remainder()
            .at_least(min)
            .resizable(false),
    };
    out.clip(true)
}

/// `rect` as AccessKit bounds.
fn bounds(rect: Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// A header's accessible node: the whole cell this pass, and its sort.
fn header_node(response: &egui::Response, whole: Rect, sorted: Option<Order>) {
    response.ctx.accesskit_node_builder(response.id, |node| {
        node.set_bounds(bounds(whole));
        if let Some(order) = sorted {
            node.set_sort_direction(match order {
                Order::Ascending => SortDirection::Ascending,
                Order::Descending => SortDirection::Descending,
            });
        }
    });
}

/// `text` laid out on one line in `font` and `colour`, cut to `width` with
/// "…" when `elide`.
fn line(
    ui: &Ui,
    text: &str,
    font: FontId,
    colour: Color32,
    width: f32,
    elide: bool,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, colour));
    job.wrap = if elide {
        TextWrapping::truncate_at_width(width.max(0.0))
    } else {
        TextWrapping::no_max_width()
    };
    ui.painter().layout_job(job)
}

/// Where a run `width` wide starts in `[left, right]` under `align`.
fn start(align: Align, left: f32, right: f32, width: f32) -> f32 {
    match align {
        Align::Start => left,
        Align::Center => left + ((right - left - width) / 2.0).max(0.0),
        Align::End => (right - width).max(left),
    }
}

/// A header cell: its label (strong while sorted), the sort arrow after it,
/// the hover fill when it sorts, the rule under the header and, unless it
/// is the last column, a short divider at its end.
fn header_cell(
    ui: &mut Ui,
    column: &Column,
    label: &str,
    sorted: Option<Order>,
    pad: f32,
    rule: Stroke,
    divider: bool,
) {
    let rect = ui.max_rect();
    let visuals = ui.visuals().clone();
    let gap = ui.spacing().item_spacing.x;
    let whole = rect.expand2(vec2(gap / 2.0, 0.0));
    // The cell clips to itself; the fill and rules reach across the gaps.
    let mut across = ui.painter().clone();
    across.set_clip_rect(whole.expand(1.0));
    if column.sortable && ui.rect_contains_pointer(rect) {
        across.rect_filled(whole, 0.0, visuals.widgets.hovered.bg_fill);
    }
    let colour = if sorted.is_some() {
        visuals.strong_text_color()
    } else {
        visuals.weak_text_color()
    };
    let font = TextStyle::Body.resolve(ui.style());
    let side = font.size.round();
    let arrow = if sorted.is_some() { side + pad } else { 0.0 };
    let (left, right) = (rect.left() + pad, rect.right() - pad);
    let galley = line(ui, label, font, colour, right - left - arrow, true);
    let x = start(column.align, left, right, galley.size().x + arrow);
    let y = rect.center().y - galley.size().y / 2.0;
    let text_right = x + galley.size().x;
    ui.painter().galley(pos2(x, y), galley, colour);
    if let Some(order) = sorted {
        let icon = match order {
            Order::Ascending => Icon::ArrowUp,
            Order::Descending => Icon::ArrowDown,
        };
        let centre = pos2(text_right + pad + side / 2.0, rect.center().y);
        paint_icon(ui, icon, centre, side, colour);
    }
    across.hline(whole.x_range(), rect.bottom() - 0.5, rule);
    if divider {
        let inset = rect.height() / 4.0;
        across.vline(
            whole.right(),
            (rect.top() + inset)..=(rect.bottom() - inset),
            rule,
        );
    }
}

/// A body cell's text and marker icons; `true` when the text was elided.
fn paint_cell(
    ui: &mut Ui,
    cell: &Cell,
    column: &Column,
    style: RowStyle,
    selected: bool,
    pad: f32,
) -> bool {
    let rect: Rect = ui.max_rect();
    let visuals = ui.visuals();
    let colour = if selected {
        visuals.selection.stroke.color
    } else if style.weak || cell.weak {
        visuals.weak_text_color()
    } else if style.strong {
        visuals.strong_text_color()
    } else {
        visuals.text_color()
    };
    let marker = if selected {
        colour
    } else {
        visuals.weak_text_color()
    };
    let body = TextStyle::Body.resolve(ui.style());
    let font = if style.strong {
        crate::fonts::bound(ui.ctx(), crate::fonts::medium(body.size))
    } else {
        body
    };
    let side = font.size.round();
    let icons = cell.icons.len() as f32 * (side + pad);
    let (left, right) = (rect.left() + pad, rect.right() - pad - icons);
    let galley = line(ui, &cell.text, font, colour, right - left, column.elide);
    let elided = galley.elided;
    let x = start(column.align, left, right, galley.size().x);
    let y = rect.center().y - galley.size().y / 2.0;
    ui.painter().galley(pos2(x, y), galley, colour);
    for (n, icon) in cell.icons.iter().enumerate() {
        let centre = pos2(
            rect.right() - pad - side / 2.0 - n as f32 * (side + pad),
            rect.center().y,
        );
        paint_icon(ui, *icon, centre, side, marker);
    }
    elided
}
