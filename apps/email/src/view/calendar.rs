// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The calendar surface: an account's selected calendar's events in the
//! centre pane and the open event's detail in the reader (the calendars
//! themselves are the tree's). The detail renders the indexed title and
//! start/end columns, with the JSCalendar description when the event
//! carries one.

use chrono::{DateTime, Utc};
use egui::{RichText, ScrollArea, Ui};
use jmap::CalendarEvent;
use toolkit::{Icon, icons, panel};

use super::UiEvent;
use crate::state::AccountView;

/// The centre pane's event list.
pub fn list(ui: &mut Ui, view: &AccountView, events: &mut Vec<UiEvent>) {
    let title = view
        .calendars
        .calendars
        .iter()
        .find(|calendar| Some(calendar.id.as_str()) == view.calendars.selected.as_deref())
        .map(|calendar| calendar.name.clone())
        .unwrap_or_else(|| crate::label("calendar"));
    panel::group(ui, "events", &[title.as_str()], |ui, _| {
        if view.calendars.busy {
            ui.weak(crate::label("loading"));
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if view.calendars.events.is_empty() && !view.calendars.busy {
                ui.weak(crate::label("empty"));
            }
            for (index, event) in view.calendars.events.iter().enumerate() {
                row(ui, index, event, view, events);
            }
        });
    });
}

/// One event row: title, the start, the range.
fn row(
    ui: &mut Ui,
    index: usize,
    event: &CalendarEvent,
    view: &AccountView,
    events: &mut Vec<UiEvent>,
) {
    let selected = view.calendars.open == Some(index);
    egui::Sides::new().show(
        ui,
        |ui| {
            let weak = ui.visuals().weak_text_color();
            ui.add(icons::image_of(ui.ctx(), Icon::Calendar, 12.0, weak));
            let text = if selected {
                RichText::new(event.display_title()).strong()
            } else {
                RichText::new(event.display_title())
            };
            if toolkit::selectable(ui, selected, text).clicked() {
                events.push(UiEvent::SelectEvent(view.id().to_owned(), index));
            }
        },
        |ui| {
            ui.label(RichText::new(range_of(event)).weak());
        },
    );
}

/// The reader: the open event's detail.
pub fn reader(ui: &mut Ui, view: &AccountView) {
    let Some(event) = view
        .calendars
        .open
        .and_then(|index| view.calendars.events.get(index))
    else {
        ui.weak(crate::label("select-event"));
        return;
    };
    panel::section_label(ui, &event.display_title());
    ui.label(
        RichText::new(crate::label_with(
            "when-line",
            &[("when", &range_of(event))],
        ))
        .weak(),
    );
    let calendar = view
        .calendars
        .calendars
        .iter()
        .find(|calendar| calendar.id == event.calendar_id);
    if let Some(calendar) = calendar {
        ui.label(
            RichText::new(crate::label_with(
                "calendar-line",
                &[("calendar", &calendar.name)],
            ))
            .weak(),
        );
    }
    toolkit::bars::hairline(ui);
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        // The JSCalendar description, when the event carries one.
        if let Some(description) = event
            .data
            .get("description")
            .and_then(serde_json::Value::as_str)
        {
            ui.add(egui::Label::new(description).wrap());
        } else {
            ui.weak(crate::label("no-description"));
        }
    });
}

/// An event's readable range: "14 Oct 09:00 – 10:30" style.
fn range_of(event: &CalendarEvent) -> String {
    let start = event.starts_at();
    let end = event.ends_at();
    let fmt = |dt: &DateTime<Utc>| dt.format("%d %b %H:%M").to_string();
    match (start, end) {
        (Some(start), Some(end)) => format!("{} – {}", fmt(&start), fmt(&end)),
        (Some(start), None) => fmt(&start),
        (None, _) => String::new(),
    }
}
