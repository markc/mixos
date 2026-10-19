// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The mail surface: an account's mailbox list in the centre pane and the
//! open email in the reader (the mailboxes themselves are the tree's).

use std::fmt::Write as _;

use chrono::{DateTime, Datelike, FixedOffset, Local, TimeZone};
use egui::{RichText, ScrollArea, Ui};
use jmap::Email;
use toolkit::button::{IconButton, Link};
use toolkit::command::{Origin, label_text};
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
    _commands: &Registry<State>,
    strings: &Strings,
    _stroke: f32,
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
            .strings(strings)
            .selection(SelectionMode::Multi)
            .total(Total::Known(view.mail.total))
            .select_everything(everything)
            .selected_count(view.mail.everything.then(|| view.mail.selection_count()))
            .empty_text(crate::label(empty))
            .show(ui, &rows);
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
pub fn reader(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    let area = ui.max_rect();
    toolbar(ui, state, view, commands, strings, stroke, events);
    toolkit::bars::hairline(ui);
    if let Some(email) = &view.mail.open {
        message(ui, view, email);
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
fn toolbar(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    stroke: f32,
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
        // What Delete, Flag, Mark and Move apply to: the list's selection
        // or keyboard row, which need not be the email shown here.
        if let Some(line) = view.mail.target_line() {
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
            attachments_popup(ui, &clip, view, email, stroke, events);
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
    stroke: f32,
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
                    ui.add(icons::image(
                        type_icon(a.content_type.as_deref()),
                        stroke,
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

/// The open email: subject first and largest, then From, To and the date,
/// then the body.
fn message(ui: &mut Ui, view: &AccountView, email: &Email) {
    let reader = &view.mail.reader;
    // The cache is the open email's once it has loaded; until then (or for
    // an email put in place without opening), detect afresh.
    let format = if reader.is_for(email) {
        reader.format()
    } else {
        reader::detect(email)
    };
    ui.add(
        egui::Label::new(RichText::new(email.subject.as_deref().unwrap_or("")).heading()).wrap(),
    );
    ui.label(
        RichText::new(crate::label_with(
            "from-line",
            &[("from", &email.from_display())],
        ))
        .weak(),
    );
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
    if !to.is_empty() {
        ui.label(RichText::new(crate::label_with("to-line", &[("to", &to)])).weak());
    }
    ui.label(RichText::new(email.date_short()).weak());
    toolkit::bars::hairline(ui);
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
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
}
