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
use toolkit::command::Origin;
use toolkit::datatable::RowsDrag;
use toolkit::{Icon, Registry, Strings, icons, panel};

use super::UiEvent;
use crate::state::{Phase, RowKind, State, TreeKey, TreeRow};

const ICON: f32 = 12.0;

/// The tree's focus target.
fn tree_id() -> Id {
    Id::new("account-tree")
}

/// The tree panel.
pub fn show(
    ui: &mut Ui,
    state: &State,
    commands: &Registry<State>,
    strings: &Strings,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    let id = tree_id();
    let focused = ui.memory(|m| m.has_focus(id));
    if focused {
        // The arrows are the tree's, not egui's focus movement.
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter {
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
    let back = ui.interact(ui.available_rect_before_wrap(), id, Sense::click());
    if back.clicked() {
        back.request_focus();
    }
    let rows = state.tree();
    let title = crate::label("accounts-title");
    panel::group(ui, "tree", &[title.as_str()], |ui, _| {
        ScrollArea::vertical()
            .id_salt("account-tree-rows")
            .auto_shrink(false)
            .show(ui, |ui| {
                let menus = Menus {
                    state,
                    commands,
                    strings,
                };
                for row in &rows {
                    let cursor = focused && state.ui.cursor.as_ref() == Some(&row.node);
                    draw(ui, row, cursor, stroke, &menus, events);
                }
            });
    });
}

/// What a row's context menu is drawn from.
struct Menus<'a> {
    state: &'a State,
    commands: &'a Registry<State>,
    strings: &'a Strings,
}

/// How long a drag must rest on a collapsed row before it opens.
const OPEN_AFTER: f64 = 0.7;

/// One row: indent, disclosure, icon, label, unread count; its context
/// menu; and, while the list's emails are dragged, a drop target.
fn draw(
    ui: &mut Ui,
    row: &TreeRow,
    cursor: bool,
    stroke: f32,
    menus: &Menus,
    events: &mut Vec<UiEvent>,
) {
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
                    let toggle =
                        ui.add(icons::image(chevron, stroke, ICON, weak).sense(Sense::click()));
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
                ui.add(icons::image(icon, stroke, ICON, colour));
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
                let response = toolkit::selectable(ui, row.selected, text);
                if response.clicked() {
                    events.push(UiEvent::Select(row.node.clone()));
                    ui.memory_mut(|m| m.request_focus(tree_id()));
                }
                if response.secondary_clicked() {
                    events.push(UiEvent::ContextNode(row.node.clone()));
                }
                let ids = menus.state.node_menu(&row.node);
                response.context_menu(|ui| {
                    let why = |id: &str| menus.state.folder_why(id);
                    if let Some(id) =
                        menus
                            .commands
                            .context_menu(ui, menus.state, menus.strings, &ids, &why)
                    {
                        events.push(UiEvent::Command(id, Origin::Menu));
                    }
                });
            },
            |ui| {
                if row.unread > 0 {
                    ui.label(RichText::new(row.unread.to_string()).strong());
                }
            },
        )
    });
    drop_target(ui, row, shown.response.rect, menus.state, events);
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

/// While the list's emails are dragged over `rect` (row `row`): a folder
/// that takes them lights in the accent and takes the drop; any other row
/// shows the not-allowed cursor and, for another account's folder, says
/// why. A collapsed row the drag rests on opens.
fn drop_target(ui: &mut Ui, row: &TreeRow, rect: egui::Rect, state: &State, events: &mut Vec<UiEvent>) {
    let ctx = ui.ctx().clone();
    let Some(drag) = egui::DragAndDrop::payload::<RowsDrag>(&ctx) else {
        return;
    };
    if drag.table != super::mail::LIST {
        return;
    }
    let rest = egui::Id::new(("email.drop-rest", row.node.key()));
    let over = ui.rect_contains_pointer(rect);
    if !over {
        ctx.data_mut(|d| d.remove::<f64>(rest));
        return;
    }
    let now = ctx.input(|i| i.time);
    if row.expandable && !row.expanded {
        let since = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(rest, || now));
        if now - since >= OPEN_AFTER {
            events.push(UiEvent::Expand(row.node.clone(), true));
            ctx.data_mut(|d| d.remove::<f64>(rest));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
    match state.drop_refusal(&row.node) {
        None => {
            let accent = toolkit::chrome::Chrome::of(&ctx).palette.accent;
            ui.painter().rect_stroke(
                rect,
                ui.visuals().widgets.inactive.corner_radius,
                egui::Stroke::new(2.0, accent),
                egui::StrokeKind::Inside,
            );
            if ctx.input(|i| i.pointer.any_released()) {
                egui::DragAndDrop::clear_payload(&ctx);
                events.push(UiEvent::DropOnFolder(row.node.clone(), drag.keys.clone()));
            }
        }
        Some(why) => {
            ctx.set_cursor_icon(egui::CursorIcon::NotAllowed);
            if why == "drop-other-account" {
                egui::Tooltip::always_open(
                    ctx.clone(),
                    ui.layer_id(),
                    egui::Id::new("email.drop-why"),
                    egui::PopupAnchor::Pointer,
                )
                .show(|ui| ui.label(crate::label(why)));
            }
        }
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
