// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The account tree, Thunderbird's shape: each account a root (its name,
//! its connection state, its total unread), and under it the mailboxes
//! nested by parent with their unread counts, then a Calendars group and an
//! Address Books group. The rows, their expand state and the keyboard
//! cursor are the engine's ([`State::tree`]); this draws them and reports
//! clicks, disclosures and keys.
//!
//! The tree takes the keyboard once a row is clicked: Up and Down move the
//! cursor, Left collapses (or goes to the parent), Right expands (or goes
//! to the first child), Enter selects.
//!
//! The toolkit has no tree widget to extend, so this one stays the app's
//! own (AGENTS.md §2.2) until a second owner needs it.

use egui::{EventFilter, Id, Key, Modifiers, RichText, ScrollArea, Sense, Ui};
use toolkit::{Icon, icons, panel};

use super::UiEvent;
use crate::state::{Phase, RowKind, State, TreeKey, TreeRow};

const ICON: f32 = 12.0;

/// The tree's focus target.
fn tree_id() -> Id {
    Id::unique("account-tree")
}

/// Give the tree the keyboard.
pub fn focus(ctx: &egui::Context) {
    ctx.memory_mut(|m| m.request_focus(tree_id()));
}

/// Whether the tree has the keyboard.
pub fn is_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.has_focus(tree_id()))
}

/// The tree panel.
pub fn show(ui: &mut Ui, state: &State, events: &mut Vec<UiEvent>) {
    let id = tree_id();
    let focused = ui.memory(|m| m.has_focus(id));
    if focused {
        // The arrows are the tree's, not egui's focus movement.
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..EventFilter::default()
                },
            )
        });
        for (key, press) in [
            (Key::ArrowUp, TreeKey::Up),
            (Key::ArrowDown, TreeKey::Down),
            (Key::ArrowLeft, TreeKey::Left),
            (Key::ArrowRight, TreeKey::Right),
            (Key::Enter, TreeKey::Enter),
        ] {
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, key)) {
                events.push(UiEvent::TreeKey(press));
            }
        }
    }
    // The panel itself takes focus, under the rows (registered first, so a
    // row's click wins and hands the focus here).
    // Focusable but not clickable, so the header's dots keep their clicks; a
    // press on the panel hands it the keyboard.
    let area = ui.available_rect_before_wrap();
    let back = ui.interact(area, id, Sense::focusable_noninteractive());
    if ui.input(|i| i.pointer.primary_pressed()) && ui.rect_contains_pointer(area) {
        back.request_focus();
    }
    let rows = state.tree();
    let title = crate::label("accounts-title");
    panel::group(ui, "tree", &[title.as_str()], |ui, _| {
        ScrollArea::vertical()
            .id_salt("account-tree-rows")
            .auto_shrink(false)
            .show(ui, |ui| {
                for row in &rows {
                    let cursor = focused && state.ui.cursor.as_ref() == Some(&row.node);
                    draw(ui, row, cursor, events);
                }
            });
    });
}

/// One row: indent, disclosure, icon, label, unread count.
fn draw(ui: &mut Ui, row: &TreeRow, cursor: bool, events: &mut Vec<UiEvent>) {
    let weak = ui.visuals().weak_text_color();
    let error = ui.visuals().error_fg_color;
    let indent = ui.spacing().indent * row.depth as f32;
    let shown = ui.scope(|ui| {
        egui::Sides::new().show(
            ui,
            |ui| {
                ui.add_space(indent);
                if row.expandable {
                    let chevron = if row.expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    };
                    let toggle = ui
                        .add(icons::image_of(ui.ctx(), chevron, ICON, weak).sense(Sense::click()))
                        .accessible_name(crate::label_with(if row.expanded { "tree-collapse" } else { "tree-expand" }, &[("name", &row.label)]));
                    if toggle.clicked() {
                        events.push(UiEvent::Expand(row.node.clone(), !row.expanded));
                        ui.memory_mut(|m| m.request_focus(tree_id()));
                    }
                } else {
                    // Where a disclosure would be, so icons line up by depth.
                    ui.add_space(ICON + ui.spacing().item_spacing.x);
                }
                let (icon, colour) = match &row.kind {
                    RowKind::Account(Phase::Ready) => (Icon::Mail, weak),
                    RowKind::Account(Phase::Connecting) => (Icon::LoaderCircle, weak),
                    RowKind::Account(Phase::Failed(_) | Phase::NoPassword) => {
                        (Icon::CircleAlert, error)
                    }
                    RowKind::Account(Phase::Disabled) => (Icon::Unplug, weak),
                    RowKind::Mailbox(role) => (mailbox_icon(role.as_deref()), weak),
                    RowKind::CalendarGroup | RowKind::Calendar => (Icon::Calendar, weak),
                    RowKind::BookGroup | RowKind::Book => (Icon::BookUser, weak),
                };
                ui.add(icons::image_of(ui.ctx(), icon, ICON, colour));
                let root = matches!(row.kind, RowKind::Account(_));
                let text = if row.selected || root {
                    RichText::new(&row.label).strong()
                } else {
                    RichText::new(&row.label)
                };
                let text = if matches!(row.kind, RowKind::Account(Phase::Disabled)) {
                    text.weak()
                } else {
                    text
                };
                if toolkit::selectable(ui, row.selected, text).clicked() {
                    events.push(UiEvent::Select(row.node.clone()));
                    ui.memory_mut(|m| m.request_focus(tree_id()));
                }
            },
            |ui| {
                if row.unread > 0 {
                    ui.label(RichText::new(row.unread.to_string()).strong());
                }
            },
        )
    });
    if cursor {
        let rect = shown.response.rect;
        let visuals = ui.visuals();
        ui.painter().rect_stroke(
            rect,
            visuals.widgets.inactive.corner_radius,
            visuals.selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
}

/// A mailbox's icon by its role.
fn mailbox_icon(role: Option<&str>) -> Icon {
    match role {
        Some("inbox") => Icon::Inbox,
        Some("sent") => Icon::Send,
        Some("drafts") => Icon::FileText,
        Some("trash") => Icon::Trash,
        Some("junk") | Some("spam") => Icon::CircleAlert,
        Some("archive") | Some("all") => Icon::FileArchive,
        _ => Icon::Folder,
    }
}
