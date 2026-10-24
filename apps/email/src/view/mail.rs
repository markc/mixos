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
    panel::group(ui, "emails", &[title.as_str(), count.as_str()], |ui, _| {
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
        let menu = ui.id().with("list-menu");
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
    egui::Id::new("email.clock")
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
        toolbar(ui, view, events);
        toolkit::bars::hairline(ui);
    }
    // Only the message body takes the reader's own style (its padding, font
    // and light mode, from the profile); the header keeps the window's look.
    if let Some(email) = &view.mail.open {
        message(ui, view, email, focused, &state.settings.reader);
    } else {
        ui.weak(crate::label("select-email"));
    }
    let menu = ui.id().with("reader-menu");
    if let Some(id) = commands.context_menu_over(ui, area, menu, "message", state, strings) {
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

/// The row under the header: the target line when several messages are
/// selected, and the attachments when the message has any.
/// Whether the toolbar row has anything in it: the target line for several
/// selected messages, or the open email's attachments.
fn toolbar_shown(view: &AccountView) -> bool {
    let several = view.mail.selection_count() > 1 && view.mail.target_line().is_some();
    let attached = view
        .mail
        .open
        .as_ref()
        .and_then(|e| e.attachments.as_ref())
        .is_some_and(|list| !list.is_empty());
    several || attached
}

fn toolbar(ui: &mut Ui, view: &AccountView, events: &mut Vec<UiEvent>) {
    ui.horizontal(|ui| {
        // What Delete, Flag, Mark and Move apply to, when it is several
        // messages from the list: the open email alone needs no note.
        if view.mail.selection_count() > 1
            && let Some(line) = view.mail.target_line()
        {
            ui.label(RichText::new(line).weak());
        }
        // The paperclip and its count, only when there are attachments.
        let attached = view
            .mail
            .open
            .as_ref()
            .and_then(|e| e.attachments.as_ref())
            .filter(|list| !list.is_empty());
        if let (Some(list), Some(email)) = (attached, view.mail.open.as_ref()) {
            toolkit::bars::divider(ui, DIVIDER);
            let count = list.len().to_string();
            let clip = ui
                .add(Link::new(Icon::Paperclip, count.as_str()))
                .on_hover_text(crate::label_with("attachment-count", &[("count", &count)]));
            if clip.clicked() {
                events.push(UiEvent::Command("mail.attachments", Origin::Ui));
            }
            attachments_popup(ui, &clip, view, email, events);
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

/// The attachments popup under the paperclip: each attachment's type,
/// name (elided, whole on hover) and size, with Save and Open; Save All
/// at the foot. A part that could not be decoded is greyed with a note;
/// a program or script offers Save only.
fn attachments_popup(
    ui: &Ui,
    clip: &egui::Response,
    view: &AccountView,
    email: &Email,
    events: &mut Vec<UiEvent>,
) {
    let open = view.mail.attachments_open;
    let shown = egui::Popup::from_response(clip)
        .id(clip.id.with("attachments"))
        .open(open)
        .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
        .show(|ui| {
            let body = ui.text_style_height(&egui::TextStyle::Body);
            let name_width = body * 16.0;
            ui.set_min_width(body * 26.0);
            ui.label(RichText::new(crate::label("attachments-title")).strong());
            let weak = ui.visuals().weak_text_color();
            for (index, a) in email.attachments.iter().flatten().enumerate() {
                let name = crate::attachments::file_name(
                    a.name.as_deref(),
                    index + 1,
                    a.content_type.as_deref(),
                );
                let part = a.part_id.clone().unwrap_or_default();
                let usable = a.downloadable();
                ui.horizontal(|ui| {
                    let ink = if usable {
                        ui.visuals().text_color()
                    } else {
                        weak
                    };
                    ui.add(icons::image_of(
                        ui.ctx(),
                        type_icon(a.content_type.as_deref()),
                        14.0,
                        ink,
                    ));
                    // Fixed name and size columns, so the actions line up.
                    let height = ui.spacing().interact_size.y;
                    ui.allocate_ui(egui::vec2(name_width, height), |ui| {
                        ui.set_min_width(name_width);
                        let label = RichText::new(&name).color(ink);
                        ui.add(egui::Label::new(label).truncate())
                            .on_hover_text(a.name.as_deref().unwrap_or(&name));
                    });
                    let size = RichText::new(crate::attachments::human_size(a.size)).weak();
                    ui.allocate_ui_with_layout(
                        egui::vec2(body * 4.0, height),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(body * 4.0);
                            ui.label(size);
                        },
                    );
                    if !usable {
                        ui.label(
                            RichText::new(crate::label("attachment-undecodable"))
                                .weak()
                                .small(),
                        );
                        return;
                    }
                    if ui
                        .add(Link::new(Icon::Download, crate::label("save")))
                        .clicked()
                    {
                        events.push(UiEvent::SaveAttachment(part.clone()));
                    }
                    let save_only =
                        crate::attachments::refuses_open(&name, a.content_type.as_deref());
                    let opened = ui.add_enabled(
                        !save_only,
                        Link::new(Icon::ExternalLink, crate::label("open")),
                    );
                    let opened = if save_only {
                        opened.on_disabled_hover_text(crate::label("attachment-save-only"))
                    } else {
                        opened
                    };
                    if opened.clicked() {
                        events.push(UiEvent::OpenAttachment(part.clone()));
                    }
                });
                if let Some(transfer) = view.mail.transfers.get(&part) {
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
                    ui.label(RichText::new(note).weak().small());
                }
            }
            ui.separator();
            let any = email.attachments.iter().flatten().any(|a| a.downloadable());
            if ui
                .add_enabled(any, Link::new(Icon::Download, crate::label("save-all")))
                .clicked()
            {
                events.push(UiEvent::Command("mail.save_attachments", Origin::Ui));
            }
        });
    // A click outside the popup and the paperclip closes it.
    if open
        && let Some(shown) = shown
        && ui.input(|i| i.pointer.any_click())
        && let Some(at) = ui.input(|i| i.pointer.interact_pos())
        && !shown.response.rect.contains(at)
        && !clip.rect.contains(at)
    {
        events.push(UiEvent::AttachmentsOpen(false));
    }
}

/// The reader's focus target. Tab reaches it; while it has the keyboard, Up
/// and Down scroll the message a line and Page Up and Page Down a page.
pub fn reader_id() -> Id {
    Id::new("email.reader")
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
fn scroll_with_keys(ui: &mut Ui, focused: bool) {
    if !focused {
        return;
    }
    let body = ui.make_persistent_id(BODY_SCROLL);
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
        && let Some(mut scroll) = egui::scroll_area::State::load(ui.ctx(), body)
    {
        scroll.offset.y = (scroll.offset.y + step).max(0.0);
        scroll.store(ui.ctx(), body);
    }
}

/// The open email: subject first and largest, then From, To and the date,
/// then the body.
fn message(
    ui: &mut Ui,
    view: &AccountView,
    email: &Email,
    focused: bool,
    content: &settings::content::Content,
) {
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
    toolkit::content::surface(ui, content, |ui| {
        // The keys and the area share this ui, so they share the scroll state.
        scroll_with_keys(ui, focused);
        ScrollArea::vertical()
            .id_salt(BODY_SCROLL)
            .auto_shrink(false)
            .show(ui, |ui| {
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
            });
    });
}
