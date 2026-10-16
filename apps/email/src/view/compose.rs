// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The compose dialog: a modal over the whole window with the address
//! fields, the subject, the body, and Send/Cancel. Send runs the state's
//! recipe (create in Sent, submit); a failure keeps the dialog open with
//! the error so nothing typed is lost.
//!
//! The address fields complete from the contacts and the recent
//! correspondents, in a dropdown under the field being typed in (Up and
//! Down move, Enter or Tab take, Esc closes). A quiet line says where the
//! draft stands (saving, saved at, or why not), and closing with unsaved
//! work asks Save draft / Discard / Cancel, Cancel the default.

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Align, Id, Key, Modifiers, Response, TextEdit, Ui};
use toolkit::dialog::{self, Choice};
use toolkit::field;

use super::UiEvent;
use crate::state::drafts::CloseChoice;
use crate::state::suggest::{self, Suggest, SuggestKey, Suggestion};
use crate::state::{ComposeField, ComposeState, State};

/// The compose dialog, when a draft is open.
pub fn dialog(ui: &mut Ui, state: &State, events: &mut Vec<UiEvent>) {
    let id = egui::Id::new("compose");
    let Some(compose) = &state.compose else {
        // Gone: the next one opens centred.
        dialog::Dialog::reset(ui.ctx(), id);
        return;
    };
    let mut send = Choice::new(crate::label("send"), dialog::Role::Default);
    send.enabled = state.compose_sendable();
    let choices = vec![
        send,
        Choice::new(crate::label("cancel"), dialog::Role::Cancel),
    ];
    let send_index = 0; // choices[0] is Send; the cancel closes below.
    let shown = dialog::Dialog::new(id, crate::label("compose-title"))
        .buttons(choices)
        .show(ui.ctx(), |ui| {
            form(ui, compose, state, events);
        });
    if shown.chosen == Some(send_index) {
        events.push(UiEvent::SendCompose);
    } else if shown.chosen.is_some() || shown.dismissed {
        events.push(UiEvent::CloseCompose);
    }
    if compose.closing {
        close_prompt(ui, events);
    }
}

/// Save draft / Discard / Cancel, over the compose dialog. Cancel is the
/// default (Enter), and Esc cancels too.
fn close_prompt(ui: &mut Ui, events: &mut Vec<UiEvent>) {
    let id = egui::Id::new("compose-close");
    let choices = vec![
        Choice::new(crate::label("cancel"), dialog::Role::Default),
        Choice::new(crate::label("save-draft"), dialog::Role::Other),
        Choice::new(crate::label("discard"), dialog::Role::Other),
    ];
    let shown = dialog::Dialog::new(id, crate::label("close-draft-title"))
        .buttons(choices)
        .show(ui.ctx(), |ui| {
            ui.add(egui::Label::new(crate::label("close-draft-body")).wrap());
        });
    let choice = match shown.chosen {
        Some(1) => CloseChoice::Save,
        Some(2) => CloseChoice::Discard,
        Some(_) => CloseChoice::Cancel,
        None if shown.dismissed => CloseChoice::Cancel,
        None => return,
    };
    dialog::Dialog::reset(ui.ctx(), id);
    events.push(UiEvent::ComposeClose(choice));
}

/// The dialog's fields. Each edit is one event; the state owns the draft.
fn form(ui: &mut Ui, compose: &ComposeState, state: &State, events: &mut Vec<UiEvent>) {
    let width = ui.available_width();
    let line =
        |ui: &mut Ui, label: &str, field: ComposeField, value: &str, events: &mut Vec<UiEvent>| {
            let mut buffer = value.to_owned();
            let response = field::text(ui, &mut buffer, &crate::label(label), width);
            if response.changed() {
                events.push(UiEvent::ComposeField(field, buffer));
            }
        };
    // While a send is in flight every field is read-only: the dialog shows
    // exactly what is being sent.
    ui.add_enabled_ui(!compose.busy, |ui| {
        // From: the accounts that can send, when there is a choice.
        let senders: Vec<_> = state.accounts.iter().filter(|v| v.entry.enabled).collect();
        if senders.len() > 1 {
            let names: Vec<String> = senders
                .iter()
                .map(|v| format!("{} <{}>", v.entry.name, v.entry.email))
                .collect();
            let mut at = senders
                .iter()
                .position(|v| v.entry.id == compose.account)
                .unwrap_or(0);
            ui.horizontal(|ui| {
                ui.label(crate::label("from"));
                if toolkit::combo::show(ui, "compose-from", &mut at, &names, Some(width * 0.8))
                    .changed()
                {
                    events.push(UiEvent::ComposeAccount(senders[at].entry.id.clone()));
                }
            });
        }
        for (label, field, value) in [
            ("to", ComposeField::To, &compose.to_text),
            ("cc", ComposeField::Cc, &compose.cc_text),
            ("bcc", ComposeField::Bcc, &compose.bcc_text),
        ] {
            address(ui, compose, label, field, value, width, events);
        }
        line(
            ui,
            "subject",
            ComposeField::Subject,
            &compose.draft.subject,
            events,
        );
        let mut body = compose.draft.body.clone();
        let editor = TextEdit::multiline(&mut body)
            .desired_rows(8)
            .desired_width(f32::INFINITY)
            .hint_text(crate::label("body"));
        if ui.add(editor).changed() {
            events.push(UiEvent::ComposeField(ComposeField::Body, body));
        }
    });
    // The account's login changed under this draft: show the new From and
    // let the person take it; Send waits until they do.
    if let Some(from) = &compose.stale {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(crate::label_with(
                    "compose-from-new",
                    &[("old", &compose.draft.from), ("new", &from.address)],
                ))
                .color(ui.visuals().warn_fg_color),
            );
            let take = toolkit::button::PushButton::secondary(crate::label("use-new-from"));
            if ui.add(take).clicked() {
                events.push(UiEvent::ComposeUseNewFrom);
            }
        });
    }
    if let Some(error) = &compose.error {
        ui.label(
            egui::RichText::new(crate::label_with("send-failed", &[("error", error)]))
                .color(ui.visuals().error_fg_color),
        );
    }
    if compose.busy {
        ui.weak(crate::label("sending"));
    } else if let Some(line) = state.draft_line() {
        ui.weak(line);
    }
}

/// The id of an address field's text edit: stable, so its focus can be
/// read before it is drawn.
fn field_id(label: &str) -> Id {
    Id::new(("compose-address", label))
}

/// An address field: a text field, and while its entry has suggestions,
/// the dropdown under it. The dropdown's keys are taken before the field
/// sees them, and only while the field has the keyboard.
fn address(
    ui: &mut Ui,
    compose: &ComposeState,
    label: &str,
    field: ComposeField,
    value: &str,
    width: f32,
    events: &mut Vec<UiEvent>,
) {
    let id = field_id(label);
    let open = compose
        .suggest
        .as_ref()
        .filter(|s| s.field == field && !s.items.is_empty());
    let focused = ui.memory(|m| m.has_focus(id));
    let mut accepted = None;
    if let Some(suggest) = open
        && focused
    {
        let key = ui.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                Some(SuggestKey::Down)
            } else if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                Some(SuggestKey::Up)
            } else if i.consume_key(Modifiers::NONE, Key::Enter)
                || i.consume_key(Modifiers::NONE, Key::Tab)
            {
                Some(SuggestKey::Accept)
            } else if i.consume_key(Modifiers::NONE, Key::Escape) {
                Some(SuggestKey::Close)
            } else {
                None
            }
        });
        if key == Some(SuggestKey::Accept) {
            accepted = suggest.items.get(suggest.cursor);
        }
        if let Some(key) = key {
            events.push(UiEvent::SuggestKey(key));
        }
    }
    let mut buffer = value.to_owned();
    let response = ui.add(
        TextEdit::singleline(&mut buffer)
            .id(id)
            .hint_text(crate::label(label))
            .desired_width(width)
            .min_size(egui::vec2(0.0, field::HEIGHT))
            .vertical_align(Align::Center),
    );
    if response.changed() {
        events.push(UiEvent::ComposeField(field, buffer));
    }
    if let Some(suggest) = open {
        if let Some(index) = dropdown(ui, &response, suggest) {
            events.push(UiEvent::SuggestAccept(index));
            accepted = suggest.items.get(index);
        }
        // The keyboard went to another field: the dropdown closes. A click
        // on a row only takes the field's focus away (egui gives it to
        // nothing), so it is not "elsewhere".
        let elsewhere = ui.memory(|m| m.focused()).is_some_and(|other| other != id);
        if accepted.is_none() && elsewhere {
            events.push(UiEvent::SuggestKey(SuggestKey::Close));
        }
    }
    if let Some(item) = accepted {
        // The field keeps the keyboard, its cursor after the accepted entry
        // (the state writes the same text after this frame).
        let text = suggest::accept(value, &item.address());
        let mut edit = TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let end = CCursor::new(text.chars().count());
        edit.cursor.set_char_range(Some(CCursorRange::one(end)));
        TextEdit::store_state(ui.ctx(), id, edit);
        ui.memory_mut(|m| m.request_focus(id));
    }
}

/// The dropdown under `field`: one row per suggestion, the highlighted one
/// selected. The row clicked, if any.
fn dropdown(ui: &Ui, field: &Response, suggest: &Suggest) -> Option<usize> {
    let mut clicked = None;
    egui::Area::new(field.id.with("suggest"))
        .order(egui::Order::Tooltip)
        .fixed_pos(field.rect.left_bottom() + egui::vec2(0.0, 2.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(field.rect.width());
                for (index, item) in suggest.items.iter().enumerate() {
                    let job = row(ui, item, &suggest.query);
                    let button = egui::Button::selectable(index == suggest.cursor, job)
                        .min_size(egui::vec2(ui.available_width(), 0.0));
                    if ui.add(button).clicked() {
                        clicked = Some(index);
                    }
                }
            });
        });
    clicked
}

/// One suggestion's text: the name bold, the address muted after it, and
/// the part matching what was typed marked.
fn row(ui: &Ui, item: &Suggestion, query: &str) -> LayoutJob {
    let body = egui::TextStyle::Body.resolve(ui.style());
    let bold = toolkit::fonts::bound(ui.ctx(), toolkit::fonts::heading(body.size));
    let strong = ui.visuals().strong_text_color();
    let weak = ui.visuals().weak_text_color();
    let mark = ui.visuals().selection.bg_fill;
    let mut job = LayoutJob::default();
    let mut push = |text: &str, font: &egui::FontId, colour| {
        let plain = TextFormat {
            font_id: font.clone(),
            color: colour,
            ..TextFormat::default()
        };
        match suggest::matched(text, query) {
            Some(hit) => {
                let marked = TextFormat {
                    background: mark,
                    ..plain.clone()
                };
                job.append(&text[..hit.start], 0.0, plain.clone());
                job.append(&text[hit.clone()], 0.0, marked);
                job.append(&text[hit.end..], 0.0, plain);
            }
            None => job.append(text, 0.0, plain),
        }
    };
    match &item.name {
        Some(name) => {
            push(name, &bold, strong);
            push("  ", &body, weak);
            push(&item.email, &body, weak);
        }
        None => push(&item.email, &body, strong),
    }
    job
}
