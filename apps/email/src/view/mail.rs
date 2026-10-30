// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The mail surface: an account's mailbox list in the centre pane and the
//! open email in the reader (the mailboxes themselves are the tree's).

use std::fmt::Write as _;

use chrono::{DateTime, Datelike, FixedOffset, Local, TimeZone};
use egui::{EventFilter, Id, Key, Modifiers, RichText, ScrollArea, Sense, Ui};
use jmap::Email;
use toolkit::action::{Origin, label_text};
use toolkit::button::{IconButton, Link};
use toolkit::datatable::{Cell, DataTable, Everything, RowStyle, Rows, SelectionMode, Total};
use toolkit::markdown::{self, Document};
use toolkit::{Icon, Registry, Strings, icons, panel};

use super::UiEvent;
use crate::reader::{self, Format};
use crate::state::{
    AccountView, COLUMN_FROM, COLUMN_SUBJECT, State, Transfer, grouped, list_columns,
};

/// The message list's target id: its drive label and focus target.
pub const LIST: &str = "emails";

/// The centre pane's email list: the selected mailbox's page in a data
/// table, one compact row per email (Subject, From, Date), newest first
/// until a header picks another sort, with the pager under it.
/// A click opens its row; Ctrl+click, Shift+click, Ctrl+A or Up and Down
/// select rows without opening them. Enter or a double-click also opens the
/// keyboard's row.
pub fn list(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    // Escape while "Select all N"'s ids load stops the fetch, before the
    // table can take it to collapse the selection.
    if view.mail.fetching.is_some()
        && state.compose.is_none()
        && state.form.is_none()
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        events.push(UiEvent::CancelFetch);
    }
    // Reply, Reply All and Forward live on the open email: the reader's
    // toolbar and the Message menu.
    let title = view
        .mail
        .selected_mailbox()
        .map(|mailbox| mailbox.name.clone())
        .unwrap_or_else(|| crate::label("mail"));
    let shown = view.mail.emails.len();
    let count = if view.mail.total > 0 {
        crate::label_with(
            "count-of",
            &[
                ("shown", &shown.to_string()),
                ("total", &view.mail.total.to_string()),
            ],
        )
    } else {
        String::new()
    };
    let mut tabs = vec![title.as_str()];
    if !count.is_empty() {
        tabs.push(count.as_str());
    }
    panel::group(ui, "emails", &tabs, |ui, _| {
        let columns = list_columns(date_width(ui));
        let table = view.mail.list_table();
        let rows = EmailRows {
            view,
            now: clock(ui.ctx()),
        };
        let empty = if view.mail.busy { "loading" } else { "empty" };
        // The pager under the rows: "101–200 of 2866", First, Previous,
        // Next and Last. After the range while several are selected: how
        // many, and, with the whole page selected, "Select all N in
        // folder" (N the query's total), then that they all are.
        let total = grouped(view.mail.total);
        let everything = Everything {
            link: crate::label_with("select-all-in-folder", &[("count", &total)]),
            done: view
                .mail
                .everything
                .then(|| crate::label_with("all-selected", &[("count", &total)])),
        };
        // One page of the server's query: a header click or the pager asks
        // the server for another (the state answers the events).
        let fired = DataTable::new(LIST, &columns, &table)
            .keeps_tab(true)
            .strings(strings)
            .selection(SelectionMode::Multi)
            .total(Total::Known(view.mail.total))
            .select_everything(everything)
            .selected_count(view.mail.everything.then(|| view.mail.selection_count()))
            .empty_text(crate::label(empty))
            .show(ui, &rows);
        // A right-click on a row opens the Message menu for it, as the
        // reader's does. The table has already made a row outside the
        // selection the selection (which opens it); each entry is its
        // registry command, enabled as the menu bar's, so Reply waits for
        // the right-clicked email to load. Only a row opens the menu, never
        // the header or the pager.
        let on_row = fired
            .iter()
            .any(|e| matches!(e, toolkit::datatable::TableEvent::ContextMenu { .. }));
        let area = if on_row {
            ui.max_rect()
        } else {
            egui::Rect::NOTHING
        };
        let menu = ui.scope_id().with("list-menu");
        if let Some(id) = commands.context_menu_over(ui, area, menu, "message", state, strings) {
            events.push(UiEvent::Command(id, Origin::Menu));
        }
        for event in fired {
            events.push(UiEvent::MailTable(view.id().to_owned(), event));
        }
    });
}

/// The shown page as the table's rows: Subject with flag and attachment
/// markers, From (the name, else the address), the compact date; unread
/// rows strong, a read row's date weak.
struct EmailRows<'a> {
    view: &'a AccountView,
    now: DateTime<FixedOffset>,
}

impl Rows for EmailRows<'_> {
    fn len(&self) -> usize {
        self.view.mail.emails.len()
    }

    fn key(&self, row: usize) -> String {
        self.view.mail.emails[row].id.clone()
    }

    fn cell(&self, row: usize, column: &str) -> Cell {
        let email = &self.view.mail.emails[row];
        match column {
            COLUMN_FROM => Cell::new(email.from_display()),
            COLUMN_SUBJECT => {
                let mut cell = Cell::new(email.subject.clone().unwrap_or_default());
                // The first marker sits at the cell's end.
                if email.has_attachment == Some(true) {
                    cell = cell.icon(Icon::Paperclip);
                }
                if email.is_flagged() {
                    cell = cell.icon(Icon::Flag);
                }
                cell
            }
            _ => Cell::new(compact_date(email, self.now)).weak(email.is_read()),
        }
    }

    fn style(&self, row: usize) -> RowStyle {
        RowStyle {
            strong: !self.view.mail.emails[row].is_read(),
            weak: false,
        }
    }
}

/// The context data a pinned clock lives under.
fn clock_id() -> egui::Id {
    egui::Id::unique("email.clock")
}

/// Draw dates as if it were `unix` (seconds) at `offset` seconds east of
/// UTC: snapshots stay still whatever the clock and the process's zone.
pub fn pin_clock(ctx: &egui::Context, unix: i64, offset: i32) {
    ctx.data_mut(|d| d.insert_temp(clock_id(), (unix, offset)));
}

/// The time dates are drawn against, in the zone they are shown in: the
/// pinned one, else now in the local zone.
fn clock(ctx: &egui::Context) -> DateTime<FixedOffset> {
    ctx.data(|d| d.get_temp::<(i64, i32)>(clock_id()))
        .and_then(|(unix, offset)| {
            FixedOffset::east_opt(offset)?
                .timestamp_opt(unix, 0)
                .single()
        })
        .unwrap_or_else(|| Local::now().fixed_offset())
}

/// When `email` arrived (else its Date header), in the list's compact
/// form against `now` and in its zone: the time today, the weekday and
/// time in the six days before, the day and month this year, else the full
/// date. The patterns are the catalogue's (`date-today`, `date-week`,
/// `date-year`, `date-older`).
pub fn compact_date(email: &Email, now: DateTime<FixedOffset>) -> String {
    let Some(when) = email
        .received_at
        .as_deref()
        .or(email.date.as_deref())
        .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
    else {
        return String::new();
    };
    let when = when.with_timezone(now.offset());
    let days = (now.date_naive() - when.date_naive()).num_days();
    let key = match days {
        0 => "date-today",
        1..=6 => "date-week",
        _ if when.year() == now.year() => "date-year",
        _ => "date-older",
    };
    format_date(&when, key)
}

/// The catalogue's date patterns, today's first.
const DATE_FORMS: [&str; 4] = ["date-today", "date-week", "date-year", "date-older"];

/// `when` in the catalogue pattern `key`; a pattern chrono refuses gives
/// the plain date rather than a panic.
fn format_date(when: &DateTime<FixedOffset>, key: &str) -> String {
    let mut out = String::new();
    if write!(out, "{}", when.format(&crate::label(key))).is_err() {
        out = when.format("%Y-%m-%d").to_string();
    }
    out
}

/// The Date column's width: its widest compact form in the strong face,
/// or its header, plus the cell padding.
fn date_width(ui: &Ui) -> f32 {
    let size = egui::TextStyle::Body.resolve(ui.style()).size;
    let font = toolkit::fonts::bound(ui.ctx(), toolkit::fonts::medium(size));
    // A wide weekday, day and month: Wednesday 28 May 2025, 23:58.
    let widest = FixedOffset::east_opt(0)
        .and_then(|utc| utc.with_ymd_and_hms(2025, 5, 28, 23, 58, 0).single());
    let colour = ui.visuals().text_color();
    let width = |text: String| {
        ui.painter()
            .layout_no_wrap(text, font.clone(), colour)
            .size()
            .x
    };
    let dates = widest
        .into_iter()
        .flat_map(|when| DATE_FORMS.map(|key| format_date(&when, key)));
    let header = width(crate::label("column-date"));
    dates.map(width).fold(header, f32::max) + ui.spacing().item_spacing.x + 2.0
}

/// The reader toolbar's split Reply menu: the commands that pick quoting.
const REPLY_OPTIONS: [&str; 4] = [
    "mail.reply_quoted",
    "mail.reply_unquoted",
    "mail.reply_all_quoted",
    "mail.reply_all_unquoted",
];

/// The divider's height between the toolbar's groups.
const DIVIDER: f32 = 16.0;

/// The reader: a toolbar of what to do with the open email, then its
/// subject, headers and body. A text body is shown as Markdown when it is
/// (or the person asked for it, View › Message), else as the plain text it
/// is. A right-click anywhere in it opens the Message menu.
/// The mail reader as a whole panel: its header carries the actions and the
/// panel menu (the three dots), and its body is the message.
pub fn panel(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let mut header = Vec::new();
    let group = toolkit::panel::group_with_actions(
        ui,
        "reader",
        |ui| actions(ui, state, commands, strings, &mut header),
        |ui, _| reader(ui, state, view, commands, strings, events),
    );
    events.extend(header);
    crate::view::reader_menu(ui, state, &group.menu, events);
}

pub fn reader(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let area = ui.max_rect();
    // Registered first, so the reader's own widgets win a click; a click on
    // the pane itself hands it the keyboard.
    let id = reader_id();
    // Focusable but not clickable, so the header's dots keep their clicks; a
    // press in the pane hands it the keyboard.
    let back = ui.interact(area, id, Sense::focusable_noninteractive());
    if ui.input(|i| i.pointer.primary_pressed()) && ui.rect_contains_pointer(area) {
        back.request_focus();
    }
    let focused = ui.memory(|m| m.has_focus(id));
    if focused {
        // Up and Down scroll the message; Left and Right do nothing here.
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter {
                    tab: true,
                    vertical_arrows: true,
                    horizontal_arrows: true,
                    ..EventFilter::default()
                },
            )
        });
    }
    // The row is only there when it has something to say; an empty row would
    // still take its height above the subject.
    if toolbar_shown(view) {
        toolbar(ui, view);
        toolkit::bars::hairline(ui);
    }
    // Only the message body takes the reader's own style (its padding, font
    // and light mode, from the profile); the header keeps the window's look.
    let mut menu_area = area;
    let mut attachment_context = false;
    if let Some(email) = &view.mail.open {
        if email
            .attachments
            .as_ref()
            .is_some_and(|parts| !parts.is_empty())
        {
            // Measure with the reader's full height, then pin an exact panel.
            // A bottom panel's previous height otherwise constrains expansion
            // (and can leave a newly expanded tile row clipped).
            let tile_limit = ui.available_height() * 0.4;
            let mut measure = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("attachment-measure")
                    .max_rect(ui.available_rect_before_wrap())
                    .layout(egui::Layout::top_down(egui::Align::Min))
                    .invisible()
                    .sizing_pass(),
            );
            attachment_bar(
                &mut measure,
                state,
                view,
                email,
                tile_limit,
                &mut Vec::new(),
            );
            let height = measure.min_rect().height().ceil().min(tile_limit);
            let bar = egui::Panel::bottom("reader-attachments")
                .frame(egui::Frame::new())
                .exact_size(height)
                .show(ui, |ui| {
                    attachment_bar(ui, state, view, email, tile_limit, events)
                });
            menu_area.max.y = bar.response.rect.top();
        }
        attachment_context = message(
            ui,
            state,
            view,
            email,
            focused,
            &state.settings.reader,
            events,
        );
    } else {
        ui.weak(crate::label("select-email"));
    }
    let menu = ui.scope_id().with("reader-menu");
    if !attachment_context
        && let Some(id) = commands.context_menu_over(ui, menu_area, menu, "message", state, strings)
    {
        events.push(UiEvent::Command(id, Origin::Menu));
    }
}

/// Reply (with its quoting menu), Reply All, Forward, then Delete apart.
/// Each is its registry command: same label, enablement and shortcut.
/// The reader's actions, which sit in its panel header beside the dots:
/// reply (with its options), reply all, forward and delete.
pub fn actions(
    ui: &mut Ui,
    state: &State,
    commands: &Registry<State>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    ui.horizontal(|ui| {
        let link = |ui: &mut Ui, events: &mut Vec<UiEvent>, id: &'static str, icon: Icon| {
            let Some(command) = commands.get(id) else {
                return;
            };
            let key = commands.label_of(id, state).unwrap_or(command.label);
            let text = label_text(strings, key);
            let mut response = ui.add_enabled((command.enabled)(state), Link::new(icon, text));
            if let Some(chord) = command.shortcut {
                response = response.on_hover_text(ui.ctx().format_shortcut(&chord));
            }
            if response.clicked() {
                events.push(UiEvent::Command(id, Origin::Ui));
            }
        };
        link(ui, events, "mail.reply", Icon::Reply);
        let replying = commands
            .get("mail.reply")
            .is_some_and(|c| (c.enabled)(state));
        let more = ui.add_enabled(
            replying,
            IconButton::new(Icon::ChevronDown)
                .size(ui.spacing().interact_size.y * 0.8)
                .tooltip(crate::label("reply-options")),
        );
        egui::Popup::menu(&more).show(|ui| {
            for id in REPLY_OPTIONS {
                let Some(command) = commands.get(id) else {
                    continue;
                };
                let button = egui::Button::new(label_text(strings, command.label));
                if ui.add_enabled((command.enabled)(state), button).clicked() {
                    events.push(UiEvent::Command(id, Origin::Ui));
                }
            }
        });
        link(ui, events, "mail.reply_all", Icon::ReplyAll);
        link(ui, events, "mail.forward", Icon::Forward);
        toolkit::bars::divider(ui, DIVIDER);
        link(ui, events, "mail.delete", Icon::Trash);
    });
}

/// Only a multi-message target needs a toolbar row below the actions.
fn toolbar_shown(view: &AccountView) -> bool {
    view.mail.selection_count() > 1 && view.mail.target_line().is_some()
}

fn toolbar(ui: &mut Ui, view: &AccountView) {
    ui.horizontal(|ui| {
        if let Some(line) = view.mail.target_line() {
            ui.label(RichText::new(line).weak());
        }
    });
}

/// The type icon for an attachment of type `mime`.
fn type_icon(mime: Option<&str>) -> Icon {
    let mime = mime.unwrap_or("").to_ascii_lowercase();
    match mime.split('/').next().unwrap_or("") {
        "image" => Icon::FileImage,
        "audio" => Icon::FileMusic,
        "video" => Icon::FileVideoCamera,
        "text" if mime.starts_with("text/csv") => Icon::FileSpreadsheet,
        "text" => Icon::FileText,
        _ if mime.contains("zip") || mime.contains("tar") || mime.contains("compressed") => {
            Icon::FileArchive
        }
        _ if mime.contains("spreadsheet") || mime.contains("excel") => Icon::FileSpreadsheet,
        _ if mime.contains("pdf") || mime.contains("document") || mime.contains("word") => {
            Icon::FileText
        }
        _ => Icon::File,
    }
}

/// The pinned attachment bar shares the existing transfer actions and rules.
fn attachment_bar(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    email: &Email,
    tile_limit: f32,
    events: &mut Vec<UiEvent>,
) {
    toolkit::bars::hairline(ui);
    let parts = email.attachments.as_deref().unwrap_or_default();
    let expanded = state.ui.attachments_expanded;
    let mut toggle = false;
    egui::Sides::new().show(
        ui,
        |ui| {
            ui.horizontal(|ui| {
                let icon = if expanded {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                };
                if ui
                    .add(IconButton::new(icon).tooltip(crate::label("attachments")))
                    .clicked()
                {
                    toggle = true;
                }
                ui.add(icons::image_of(
                    ui.ctx(),
                    Icon::Paperclip,
                    ui.text_style_height(&egui::TextStyle::Body),
                    ui.visuals().text_color(),
                ));
                ui.label(crate::label_with(
                    "attachment-count",
                    &[("count", &parts.len().to_string())],
                ));
                let total = parts
                    .iter()
                    .fold(0_u64, |sum, part| sum.saturating_add(part.size));
                ui.weak(crate::attachments::human_size(total));
            });
        },
        |ui| {
            // Sides lays out this group right to left: the menu is last visually.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let any = parts.iter().any(jmap::Attachment::downloadable);
                let more = ui.add(
                    IconButton::new(Icon::ChevronDown).tooltip(crate::label("attachment-actions")),
                );
                if ui
                    .add_enabled(any, egui::Button::new(crate::label("save-all")))
                    .clicked()
                {
                    events.push(UiEvent::Command("mail.save_attachments", Origin::Ui));
                }
                egui::Popup::menu(&more).show(|ui| {
                    if ui
                        .add_enabled(any, egui::Button::new(crate::label("save-all")))
                        .clicked()
                    {
                        events.push(UiEvent::Command("mail.save_attachments", Origin::Menu));
                        ui.close();
                    }
                    let allowed: Vec<_> = parts
                        .iter()
                        .enumerate()
                        .filter(|(i, part)| {
                            let name = attachment_name(part, *i);
                            part.downloadable()
                                && !crate::attachments::refuses_open(
                                    &name,
                                    part.content_type.as_deref(),
                                )
                        })
                        .collect();
                    if ui
                        .add_enabled(
                            !allowed.is_empty(),
                            egui::Button::new(crate::label("open-all")),
                        )
                        .clicked()
                    {
                        for (_, part) in allowed {
                            if let Some(id) = &part.part_id {
                                events.push(UiEvent::OpenAttachment(id.clone()));
                            }
                        }
                        ui.close();
                    }
                });
            });
        },
    );
    if toggle {
        events.push(UiEvent::Command("mail.attachments", Origin::Ui));
    }
    if expanded {
        ScrollArea::vertical()
            .id_salt("attachment-tiles")
            .max_height(tile_limit)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.with_layout(
                    egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true),
                    |ui| {
                        for (index, part) in parts.iter().enumerate() {
                            attachment_tile(ui, view, part, index, events);
                        }
                    },
                );
            });
        // Room under the last row, so the tiles' frames are never cut by
        // the window edge; measured with the rest of the bar.
        ui.add_space(ui.spacing().item_spacing.y * 2.0);
    }
}

fn attachment_name(part: &jmap::Attachment, index: usize) -> String {
    crate::attachments::file_name(
        part.name.as_deref(),
        index + 1,
        part.content_type.as_deref(),
    )
}

/// Both tiles and images use the same context actions. Save-only files
/// offer Save on a primary click; Open remains visibly disabled in the menu.
fn attachment_menu(
    response: &egui::Response,
    part: &jmap::Attachment,
    name: &str,
    events: &mut Vec<UiEvent>,
) {
    let id = part.part_id.clone().unwrap_or_default();
    let usable = part.downloadable();
    let save_only = crate::attachments::refuses_open(name, part.content_type.as_deref());
    if usable && response.clicked() {
        events.push(if save_only {
            UiEvent::SaveAttachment(id.clone())
        } else {
            UiEvent::OpenAttachment(id.clone())
        });
    }
    response.context_menu(|ui| {
        let open = ui.add_enabled(
            usable && !save_only,
            egui::Button::new(crate::label("open")),
        );
        let open = if save_only {
            open.on_disabled_hover_text(crate::label("attachment-save-only"))
        } else {
            open
        };
        if open.clicked() {
            events.push(UiEvent::OpenAttachment(id.clone()));
            ui.close();
        }
        if ui
            .add_enabled(usable, egui::Button::new(crate::label("save")))
            .clicked()
        {
            events.push(UiEvent::SaveAttachment(id.clone()));
            ui.close();
        }
    });
}

fn attachment_tile(
    ui: &mut Ui,
    view: &AccountView,
    part: &jmap::Attachment,
    index: usize,
    events: &mut Vec<UiEvent>,
) {
    let name = attachment_name(part, index);
    let body = ui.text_style_height(&egui::TextStyle::Body);
    let frame = egui::Frame::group(ui.style());
    let em = egui::TextStyle::Body.resolve(ui.style()).size;
    let width = (em * 16.0).min(ui.max_rect().width());
    // Give the wrapping layout the tile's width before creating its frame.
    let shown = ui
        .allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                frame
                    .show(ui, |ui| {
                        ui.set_width((width - frame.total_margin().sum().x).max(body));
                        ui.horizontal(|ui| {
                            let ink = if part.downloadable() {
                                ui.visuals().text_color()
                            } else {
                                ui.visuals().weak_text_color()
                            };
                            ui.add(icons::image_of(
                                ui.ctx(),
                                type_icon(part.content_type.as_deref()),
                                body,
                                ink,
                            ));
                            ui.add(egui::Label::new(RichText::new(&name).color(ink)).truncate());
                        });
                        ui.weak(crate::attachments::human_size(part.size));
                        if !part.downloadable() {
                            ui.label(
                                RichText::new(crate::label("attachment-undecodable"))
                                    .weak()
                                    .small(),
                            );
                        }
                        if let Some(transfer) = part
                            .part_id
                            .as_ref()
                            .and_then(|id| view.mail.transfers.get(id))
                        {
                            let note = match transfer {
                                Transfer::Saving => crate::label("attachment-saving"),
                                Transfer::Opening => crate::label("attachment-opening"),
                                Transfer::Saved(path) => {
                                    crate::label_with("attachment-saved", &[("path", path)])
                                }
                                Transfer::Opened(path) => {
                                    crate::label_with("attachment-opened", &[("path", path)])
                                }
                                Transfer::Failed(error) => {
                                    crate::label_with("attachment-failed", &[("error", error)])
                                }
                            };
                            ui.add(egui::Label::new(RichText::new(note).weak().small()).wrap());
                        }
                    })
                    .response
            },
        )
        .inner;
    let response = ui
        .interact(shown.rect, shown.id.with("open"), Sense::click())
        .on_hover_text(part.name.as_deref().unwrap_or(&name));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            ui.is_enabled() && part.downloadable(),
            crate::label_with(
                "attachment-tile",
                &[
                    ("name", &name),
                    ("size", &crate::attachments::human_size(part.size)),
                ],
            ),
        )
    });
    attachment_menu(&response, part, &name, events);
}

/// Remote URLs are never image sources. Only attachment blobs enter this cache.
fn inline_images(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    email: &Email,
    events: &mut Vec<UiEvent>,
) -> bool {
    if !state.ui.attachments_inline {
        return false;
    }
    let current = state.preview_owner().is_some_and(|owner| {
        owner.0 == view.id() && owner.2 == email.id && state.previews.is_for(&owner)
    });
    let mut context = false;
    for (index, part) in email.attachments.iter().flatten().enumerate() {
        if crate::previews::format(part).is_none() {
            continue;
        }
        let name = attachment_name(part, index);
        ui.horizontal(|ui| {
            let height = ui.text_style_height(&egui::TextStyle::Body);
            let (short, _) = ui.allocate_exact_size(egui::vec2(height, height), Sense::hover());
            let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
            ui.painter()
                .hline(short.x_range(), short.center().y, stroke);
            ui.add(egui::Label::new(&name).truncate())
                .on_hover_text(&name);
            let (line, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
            ui.painter().hline(line.x_range(), line.center().y, stroke);
        });
        match part
            .blob_id
            .as_deref()
            .filter(|_| current)
            .and_then(|id| state.previews.get(id))
        {
            Some(crate::previews::Preview::Shown(texture)) => {
                let pixels = texture.size_vec2();
                let natural = pixels / ui.ctx().pixels_per_point();
                let size = natural * (ui.available_width() / natural.x).min(1.0);
                let response = ui.add(
                    egui::Image::new(texture)
                        .fit_to_exact_size(size)
                        .sense(Sense::click())
                        .alt_text(crate::label_with("preview-image", &[("name", &name)])),
                );
                context |= response.secondary_clicked();
                attachment_menu(&response, part, &name, events);
                context |= response.context_menu_opened();
            }
            Some(crate::previews::Preview::Failed(reason)) => {
                ui.weak(crate::label_with(
                    "preview-failed",
                    &[("name", &name), ("reason", reason)],
                ));
            }
            _ => {
                ui.weak(crate::label_with("preview-loading", &[("name", &name)]));
            }
        }
    }
    context
}

/// The reader's focus target. Tab reaches it; while it has the keyboard, Up
/// and Down scroll the message a line and Page Up and Page Down a page.
pub fn reader_id() -> Id {
    Id::unique("email.reader")
}

/// Give the reader the keyboard.
pub fn focus_reader(ctx: &egui::Context) {
    ctx.memory_mut(|m| m.request_focus(reader_id()));
}

/// Whether the reader has the keyboard.
pub fn reader_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.has_focus(reader_id()))
}

/// The message body's scroll area, by its salt.
const BODY_SCROLL: &str = "reader-body";
/// One line of the body, in points, for an arrow key.
const LINE: f32 = 48.0;

/// Scroll the body with the keys, before its scroll area draws, when the
/// reader has the keyboard. The area keeps its offset under its own id.
fn scroll_with_keys(ui: &mut Ui, focused: bool) -> Option<f32> {
    if !focused {
        return None;
    }
    // Use the area's returned id rather than reproducing egui's salt hashing.
    let body = ui.data(|d| d.get_temp::<Id>(ui.make_persistent_id(BODY_SCROLL)))?;
    let page = ui.available_height() * 0.9;
    let step = ui.input_mut(|i| {
        let mut step = 0.0;
        if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
            step += LINE;
        }
        if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
            step -= LINE;
        }
        if i.consume_key(Modifiers::NONE, Key::PageDown) {
            step += page;
        }
        if i.consume_key(Modifiers::NONE, Key::PageUp) {
            step -= page;
        }
        step
    });
    if step != 0.0
        && let Some(scroll) = egui::scroll_area::State::load(ui.ctx(), body)
    {
        return Some((scroll.clamped_offset().y + step).max(0.0));
    }
    None
}

/// The open email: subject first and largest, then From, To and the date,
/// then the body.
fn message(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    email: &Email,
    focused: bool,
    content: &settings::content::Content,
    events: &mut Vec<UiEvent>,
) -> bool {
    let reader = &view.mail.reader;
    // The cache is the open email's once it has loaded; until then (or for
    // an email put in place without opening), detect afresh.
    let format = if reader.is_for(email) {
        reader.format()
    } else {
        reader::detect(email)
    };
    // Subject with the date right-aligned on its row.
    egui::Sides::new().wrap().show(
        ui,
        |ui| {
            ui.add(
                egui::Label::new(RichText::new(email.subject.as_deref().unwrap_or("")).heading())
                    .wrap(),
            );
        },
        |ui| {
            ui.label(RichText::new(email.date_short()).weak());
        },
    );
    // From with the recipients right-aligned on its row.
    let to = email
        .to
        .as_ref()
        .map(|to| {
            to.iter()
                .map(|a| a.email.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    egui::Sides::new().wrap().show(
        ui,
        |ui| {
            ui.label(
                RichText::new(crate::label_with(
                    "from-line",
                    &[("from", &email.from_display())],
                ))
                .weak(),
            );
        },
        |ui| {
            if !to.is_empty() {
                ui.label(RichText::new(crate::label_with("to-line", &[("to", &to)])).weak());
            }
        },
    );
    toolkit::bars::hairline(ui);
    let mut attachment_context = false;
    let fill = if content.light {
        toolkit::chrome::Chrome::for_theme(toolkit::content::light_theme()).palette.canvas
    } else {
        ui.visuals().panel_fill
    };
    toolkit::content::surface_filled(ui, content, fill, |ui| {
        // The keys use the actual id returned by this area's previous frame.
        let offset = scroll_with_keys(ui, focused);
        let area = ScrollArea::vertical();
        let area = if let Some(offset) = offset {
            area.vertical_scroll_offset(offset)
        } else {
            area
        };
        let shown = area.id_salt(BODY_SCROLL).auto_shrink(false).show(ui, |ui| {
            match email.text_body_value() {
                Some(body) if format == Format::Markdown => {
                    let parsed;
                    let document = match reader.document(email) {
                        Some(document) => document,
                        None => {
                            parsed = Document::parse(body);
                            &parsed
                        }
                    };
                    markdown::show(ui, document);
                }
                Some(body) => {
                    ui.add(egui::Label::new(body).wrap());
                }
                None => match email.html_body_value() {
                    // No HTML rendering in this slice: the source, escaped by
                    // the label, with a note.
                    Some(html) => {
                        ui.weak(crate::label("html-source"));
                        ui.add(egui::Label::new(egui::RichText::new(html).monospace()).wrap());
                    }
                    None => {
                        ui.weak(crate::label("no-body"));
                    }
                },
            }
            attachment_context = inline_images(ui, state, view, email, events);
        });
        ui.data_mut(|d| d.insert_temp(ui.make_persistent_id(BODY_SCROLL), shown.id));
    });
    attachment_context
}
