// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The mail surface: an account's mailbox list in the centre pane and the
//! open email in the reader (the mailboxes themselves are the tree's).

use std::fmt::Write as _;

use chrono::{DateTime, Datelike, FixedOffset, Local, TimeZone};
use egui::{RichText, ScrollArea, Ui};
use jmap::Email;
use toolkit::button::{Link, PushButton};
use toolkit::command::Origin;
use toolkit::datatable::{Cell, DataTable, RowStyle, Rows};
use toolkit::markdown::{self, Document};
use toolkit::{Icon, Registry, Strings, panel};

use super::UiEvent;
use crate::reader::{self, Format};
use crate::state::{
    AccountView, COLUMN_FOLDER, COLUMN_FROM, COLUMN_SUBJECT, Scope, State, list_columns,
};

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
        header(ui, state, view, commands, strings, events);
        let search = &view.mail.search;
        let columns = list_columns(date_width(ui), search.all());
        // The list's own state, with the remembered widths, hidden columns
        // and density; an email opened some other way shows as selected.
        let table = state.list_table(view);
        let rows = EmailRows {
            view,
            now: clock(ui.ctx()),
        };
        let empty = if view.mail.busy {
            "loading"
        } else if search.active() {
            "no-match"
        } else {
            "empty"
        };
        // The row menu: the message commands, run on the selection.
        let mut message = commands.ids_in("message");
        if message.is_empty() {
            message = commands
                .ids_in("mail")
                .into_iter()
                .filter(|id| *id != "mail.compose")
                .collect();
        }
        let mut chosen = None;
        let mut menu = |ui: &mut Ui, _row: usize| {
            if let Some(id) = commands.context_rows(ui, state, strings, &message, &|_| None) {
                chosen = Some(id);
            }
        };
        let dragging = |count: usize| {
            if count == 1 {
                crate::label("dragging-message")
            } else {
                crate::label_with("dragging-messages", &[("count", &count.to_string())])
            }
        };
        // "Select all N in folder" (or the search's results), once the page
        // is all selected; then how many are.
        let total = crate::state::grouped(view.mail.total);
        let link = if search.active() {
            crate::label_with("select-all-results", &[("count", &total)])
        } else {
            crate::label_with("select-all-in-folder", &[("count", &total)])
        };
        let everything = view.mail.everything.as_ref();
        let done = everything.map(|all| {
            let count = crate::state::grouped(all.len() as u64);
            crate::label_with("all-selected", &[("count", &count)])
        });
        let fired = DataTable::new(LIST, &columns, &table)
            .total(view.mail.total_rows())
            .strings(strings)
            .selection(toolkit::datatable::SelectionMode::Multi)
            .select_everything(toolkit::datatable::Everything { link, done })
            .selected_count(everything.map(|all| all.len() as u64))
            .empty_text(crate::label(empty))
            .context_menu(&mut menu)
            .draggable(&dragging)
            .show(ui, &rows);
        for event in fired {
            events.push(UiEvent::MailTable(view.id().to_owned(), event));
        }
        if state.ui.focus_list {
            toolkit::datatable::focus(ui.ctx(), LIST);
            events.push(UiEvent::ListFocused);
        }
        if let Some(id) = chosen {
            events.push(UiEvent::Command(id, Origin::Menu));
        }
    });
}

/// The message list's target id: its drive label and focus target.
pub const LIST: &str = "emails";

/// The search field's egui id.
pub fn search_id() -> egui::Id {
    egui::Id::new("email.search")
}

/// The scope dropdown's widths: room for its longest choice.
const SCOPE_WIDTH: f32 = 118.0;

/// Below this the search field is too narrow to use: Compose gives way
/// to its icon first.
const SEARCH_MIN: f32 = 160.0;

/// The list header: Compose, the count, the search field and its scope.
fn header(
    ui: &mut Ui,
    state: &State,
    view: &AccountView,
    commands: &Registry<State>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let account = view.id().to_owned();
    let search = &view.mail.search;
    let count = if search.active() {
        crate::label_with("results-count", &[("count", &view.mail.total.to_string())])
    } else {
        crate::label_with("messages-count", &[("count", &view.mail.total.to_string())])
    };
    ui.horizontal(|ui| {
        let gap = ui.spacing().item_spacing.x;
        let count_width = ui
            .painter()
            .layout_no_wrap(
                count.clone(),
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().text_color(),
            )
            .size()
            .x;
        // What gives way, in order, so the field keeps SEARCH_MIN: Compose's
        // label, then the count.
        let full = commands
            .get("mail.compose")
            .map(|c| {
                let label = toolkit::command::label_text(strings, c.label);
                let button = PushButton::primary(label).icon(Icon::SquarePen);
                (button.width(ui), button.icon_only(true).width(ui))
            })
            .unwrap_or_default();
        let room = ui.available_width() - SCOPE_WIDTH - 4.0 * gap;
        let squeezed = room - full.0 - count_width < SEARCH_MIN;
        let counted = room - full.1 - count_width >= SEARCH_MIN;
        if let Some(command) = commands.get("mail.compose") {
            let label = toolkit::command::label_text(strings, command.label);
            let button = PushButton::primary(label)
                .icon(Icon::SquarePen)
                .icon_only(squeezed);
            let tooltip = toolkit::tooltip::for_command(ui.ctx(), command, strings);
            let response = ui
                .add_enabled((command.enabled)(state), button)
                .on_hover_text(tooltip);
            if response.clicked() {
                events.push(UiEvent::Command(command.id, Origin::Ui));
            }
        }
        if !squeezed || counted {
            ui.label(RichText::new(count).weak());
        }
        let width = (ui.available_width() - SCOPE_WIDTH - gap).max(SEARCH_MIN / 2.0);
        let mut text = search.text.clone();
        let (field, cleared) = ui
            .push_id(search_id(), |ui| {
                toolkit::field::search_clearable(ui, &mut text, &crate::label("search-hint"), width)
            })
            .inner;
        search_keys(ui, state, &field, &account, &text, events);
        if cleared {
            events.push(UiEvent::SearchClear(account.clone()));
            field.request_focus();
        } else if field.changed() {
            events.push(UiEvent::SearchText(account.clone(), text));
        }
        let options = [
            crate::label("search-this-folder"),
            crate::label("search-all-folders"),
        ];
        let mut scope = usize::from(search.scope == Scope::All);
        if toolkit::combo::show(
            ui,
            "email.search-scope",
            &mut scope,
            &options,
            Some(SCOPE_WIDTH),
        )
        .changed()
        {
            let scope = if scope == 1 {
                Scope::All
            } else {
                Scope::Folder
            };
            events.push(UiEvent::SearchScope(account, scope));
        }
    });
}

/// The search field's keys: asked-for focus (`view.search`, or `/` from
/// the list), Escape clears it or (empty) lets go, Down goes to the list.
fn search_keys(
    ui: &mut Ui,
    state: &State,
    field: &egui::Response,
    account: &str,
    text: &str,
    events: &mut Vec<UiEvent>,
) {
    let ctx = ui.ctx().clone();
    let elsewhere =
        ctx.memory(|m| m.focused().is_none()) || toolkit::datatable::is_focused(&ctx, LIST);
    if state.ui.focus_search {
        field.request_focus();
        events.push(UiEvent::SearchFocused);
    } else if elsewhere && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Slash))
    {
        field.request_focus();
    }
    // egui lets go of a field on Escape: with text in it, clear and keep it.
    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) && !text.is_empty() {
        events.push(UiEvent::SearchClear(account.to_owned()));
        field.request_focus();
    }
    if field.has_focus()
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
    {
        toolkit::datatable::focus(&ctx, LIST);
        let first = state
            .account(account)
            .and_then(|v| v.mail.emails.first())
            .map(|e| e.id.clone());
        let none = state
            .account(account)
            .is_some_and(|v| v.mail.table.selection.is_empty());
        if let Some(key) = first.filter(|_| none) {
            events.push(UiEvent::MailTable(
                account.to_owned(),
                toolkit::datatable::TableEvent::Selected {
                    index: 0,
                    key,
                    pick: toolkit::datatable::Pick::Only,
                },
            ));
        }
    }
}

/// The shown page as the table's rows: From (the name, else the address),
/// Subject with flag and attachment markers, the compact date; unread rows
/// strong, a read row's date weak.
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
            COLUMN_FOLDER => Cell::new(
                email
                    .mailbox_ids
                    .as_ref()
                    .and_then(|ids| ids.keys().find_map(|id| self.view.mailbox_name(id)))
                    .unwrap_or_default(),
            )
            .weak(true),
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
/// or its header with the sort arrow, plus the cell padding.
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
    let header = width(crate::label("column-date")) + size + ui.spacing().item_spacing.x / 2.0;
    dates.map(width).fold(header, f32::max) + ui.spacing().item_spacing.x + 2.0
}

/// The reader: the open email's headers and body. A text body is shown as
/// Markdown when it is (or the person asked for it), else as the plain text
/// it is; a link beside the date flips it.
pub fn reader(ui: &mut Ui, view: &AccountView, events: &mut Vec<UiEvent>) {
    // Several selected: the reader says how many, not one of them.
    let selected = view
        .mail
        .everything
        .as_ref()
        .map_or(view.mail.table.selection.len(), Vec::len);
    if selected > 1 {
        let count = crate::state::grouped(selected as u64);
        ui.weak(crate::label_with("messages-selected", &[("count", &count)]));
        return;
    }
    let Some(email) = &view.mail.open else {
        ui.weak(crate::label("select-email"));
        return;
    };
    let reader = &view.mail.reader;
    // The cache is the open email's once it has loaded; until then (or for
    // an email put in place without opening), detect afresh.
    let format = if reader.is_for(email) {
        reader.format()
    } else {
        reader::detect(email)
    };
    let switchable = reader::has_text(email) && (format == Format::Markdown || !reader.auto());
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
    let date = RichText::new(email.date_short()).weak();
    if switchable {
        egui::Sides::new().show(
            ui,
            |ui| {
                ui.label(date);
            },
            |ui| {
                let (icon, key) = match format {
                    Format::Markdown => (Icon::FileCode, "show-source"),
                    Format::Plain => (Icon::FileText, "render-markdown"),
                };
                if ui.add(Link::new(icon, crate::label(key))).clicked() {
                    events.push(UiEvent::Command("view.reader.markdown", Origin::Ui));
                }
            },
        );
    } else {
        ui.label(date);
    }
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
