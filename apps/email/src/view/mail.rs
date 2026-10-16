// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The mail surface: an account's mailbox list in the centre pane and the
//! open email in the reader (the mailboxes themselves are the tree's).

use std::fmt::Write as _;

use chrono::{DateTime, Datelike, Local, TimeZone};
use egui::{RichText, ScrollArea, Ui};
use jmap::Email;
use toolkit::button::PushButton;
use toolkit::command::Origin;
use toolkit::datatable::{Cell, DataTable, RowStyle, Rows, TableState};
use toolkit::{Icon, Registry, Strings, panel};

use super::UiEvent;
use crate::state::{AccountView, COLUMN_FROM, COLUMN_SUBJECT, State, list_columns};

/// The centre pane's email list: the selected mailbox's page in a data
/// table, one compact row per email, sorted and paged by the server.
pub fn list(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let title = view
        .mail
        .selected_mailbox()
        .map(|mailbox| mailbox.name.clone())
        .unwrap_or_else(|| crate::label("mail"));
    panel::group(ui, "emails", &[title.as_str()], |ui, _| {
        // Reply works from the open email; the button sits where the reader
        // is not (a person reading in the reader panel replies from the
        // menu or the shortcut).
        ui.horizontal(|ui| {
            let Some(command) = commands.get("mail.reply") else {
                return;
            };
            let text = strings.get(command.label);
            let response = ui.add_enabled((command.enabled)(state), PushButton::secondary(text));
            if response.on_hover_text(strings.get("reply")).clicked() {
                events.push(UiEvent::Command(command.id, Origin::Ui));
            }
        });
        let columns = list_columns(date_width(ui));
        // The list's own state, with the remembered widths and density; an
        // email opened some other way (the Bus, a reply) shows as selected.
        let mut table = TableState {
            widths: state.ui.list.widths.clone(),
            density: state.ui.list.density,
            ..view.mail.table.clone()
        };
        if let Some(open) = &view.mail.open
            && table.selection.is_empty()
        {
            table.selection = vec![open.id.clone()];
        }
        let rows = EmailRows {
            emails: &view.mail.emails,
            now: clock(ui.ctx()),
        };
        let empty = if view.mail.busy { "loading" } else { "empty" };
        let fired = DataTable::new("emails", &columns, &table)
            .total(view.mail.total_rows())
            .strings(strings)
            .empty_text(crate::label(empty))
            .show(ui, &rows);
        for event in fired {
            events.push(UiEvent::MailTable(view.id().to_owned(), event));
        }
    });
}

/// The shown page as the table's rows: From (the name, else the address),
/// Subject with flag and attachment markers, the compact date; unread rows
/// strong, a read row's date weak.
struct EmailRows<'a> {
    emails: &'a [Email],
    now: DateTime<Local>,
}

impl Rows for EmailRows<'_> {
    fn len(&self) -> usize {
        self.emails.len()
    }

    fn key(&self, row: usize) -> String {
        self.emails[row].id.clone()
    }

    fn cell(&self, row: usize, column: &str) -> Cell {
        let email = &self.emails[row];
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
            strong: !self.emails[row].is_read(),
            weak: false,
        }
    }
}

/// The context data a pinned clock lives under.
fn clock_id() -> egui::Id {
    egui::Id::new("email.clock")
}

/// Draw dates as if it were `unix` (seconds): snapshots stay still.
pub fn pin_clock(ctx: &egui::Context, unix: i64) {
    ctx.data_mut(|d| d.insert_temp(clock_id(), unix));
}

/// The time dates are drawn against: the pinned one, else now.
fn clock(ctx: &egui::Context) -> DateTime<Local> {
    ctx.data(|d| d.get_temp::<i64>(clock_id()))
        .and_then(|unix| Local.timestamp_opt(unix, 0).single())
        .unwrap_or_else(Local::now)
}

/// When `email` arrived (else its Date header), in the list's compact
/// form against `now`: the time today, the weekday and time in the six
/// days before, the day and month this year, else the full date. The
/// patterns are the catalogue's (`date-today`, `date-week`, `date-year`,
/// `date-older`).
pub fn compact_date(email: &Email, now: DateTime<Local>) -> String {
    let Some(when) = email
        .received_at
        .as_deref()
        .or(email.date.as_deref())
        .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
    else {
        return String::new();
    };
    let when = when.with_timezone(&Local);
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
fn format_date(when: &DateTime<Local>, key: &str) -> String {
    let mut out = String::new();
    if write!(out, "{}", when.format(&crate::label(key))).is_err() {
        out = when.format("%Y-%m-%d").to_string();
    }
    out
}

/// The Date column's width: its widest compact form in the strong face,
/// or its header with the sort arrow, plus the cell padding.
fn date_width(ui: &Ui) -> f32 {
    let size = egui::TextStyle::Body.resolve(ui.style()).size;
    let font = toolkit::fonts::bound(ui.ctx(), toolkit::fonts::medium(size));
    // A wide weekday, day and month: Wednesday 28 May 2025, 23:58.
    let widest = Local.with_ymd_and_hms(2025, 5, 28, 23, 58, 0).single();
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
    let header = width(crate::label("column-date")) + size + ui.spacing().item_spacing.x / 2.0;
    dates.map(width).fold(header, f32::max) + ui.spacing().item_spacing.x + 2.0
}

/// The reader: the open email's headers and body.
pub fn reader(ui: &mut Ui, view: &AccountView) {
    let Some(email) = &view.mail.open else {
        ui.weak(crate::label("select-email"));
        return;
    };
    panel::section_label(ui, email.subject.as_deref().unwrap_or(""));
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
