// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The compose dialog: a modal over the whole window with the address
//! fields, the subject, the body, and Send/Cancel. Send runs the state's
//! recipe (create in Drafts, submit, file in Sent); a failure keeps the
//! dialog open with the error so nothing typed is lost.
//!
//! What is typed is kept as a local draft. Cancel, Escape or closing the
//! window over a draft with something in it asks first, keeping the draft
//! by default; a draft kept from an earlier run is offered at start.

use egui::{TextEdit, Ui};
use toolkit::dialog::{self, Choice};
use toolkit::field;
use toolkit::toggle::Toggle;

use super::UiEvent;
use crate::state::{ComposeField, ComposeState, Confirm, DraftChoice, State};

/// The close or quit question over a draft: Keep Draft (the default, and
/// Enter), Discard, or Keep Editing (Escape).
pub fn confirm(ui: &mut Ui, state: &State, events: &mut Vec<UiEvent>) {
    let Some(asked) = state.confirm else {
        return;
    };
    let (key, title, keep, discard) = match asked {
        Confirm::CloseCompose => (
            "confirm-close-draft",
            "close-draft-title",
            "keep-draft",
            "discard-draft",
        ),
        Confirm::Quit => (
            "confirm-quit-draft",
            "quit-draft-title",
            "keep-draft-quit",
            "discard-draft-quit",
        ),
    };
    let id = egui::Id::new(key);
    let choices = vec![
        Choice::new(crate::label(keep), dialog::Role::Default),
        Choice::new(crate::label(discard), dialog::Role::Other),
        Choice::new(crate::label("keep-editing"), dialog::Role::Cancel),
    ];
    let shown = dialog::Dialog::new(id, crate::label(title))
        .buttons(choices)
        .show(ui.ctx(), |ui| {
            ui.add(egui::Label::new(crate::label("close-draft-body")).wrap());
        });
    let choice = match shown.chosen {
        Some(0) => Some(DraftChoice::Keep),
        Some(1) => Some(DraftChoice::Discard),
        Some(_) => Some(DraftChoice::Back),
        None if shown.dismissed => Some(DraftChoice::Back),
        None => None,
    };
    if let Some(choice) = choice {
        dialog::Dialog::reset(ui.ctx(), id);
        events.push(UiEvent::ConfirmDraft(choice));
    }
}

/// The offer of the first kept draft, while no compose is open: Restore
/// (the default), Discard, or Later (Escape; every draft stays kept).
pub fn restore(ui: &mut Ui, state: &State, events: &mut Vec<UiEvent>) {
    if state.compose.is_some() || state.form.is_some() {
        return;
    }
    let Some(draft) = state.restore.first() else {
        return;
    };
    let id = egui::Id::new("restore-draft");
    let choices = vec![
        Choice::new(crate::label("restore-draft"), dialog::Role::Default),
        Choice::new(crate::label("discard-draft"), dialog::Role::Other),
        Choice::new(crate::label("later"), dialog::Role::Cancel),
    ];
    let subject = if draft.subject.trim().is_empty() {
        crate::label("no-subject")
    } else {
        draft.subject.clone()
    };
    let to = if draft.to.trim().is_empty() {
        crate::label("no-recipients")
    } else {
        draft.to.trim().trim_end_matches([',', ';']).to_owned()
    };
    let saved = chrono::DateTime::parse_from_rfc3339(&draft.saved_at)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%-d %b %Y, %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    let more = state.restore.len() - 1;
    let shown = dialog::Dialog::new(id, crate::label("restore-title"))
        .buttons(choices)
        .show(ui.ctx(), |ui| {
            ui.add(egui::Label::new(crate::label("restore-body")).wrap());
            ui.add_space(4.0);
            ui.strong(&subject);
            ui.label(crate::label_with("to-line", &[("to", &to)]));
            if !saved.is_empty() {
                ui.weak(crate::label_with("saved-line", &[("when", &saved)]));
            }
            if more > 0 {
                ui.weak(crate::label_with(
                    "restore-more",
                    &[("count", &more.to_string())],
                ));
            }
        });
    let choice = match shown.chosen {
        Some(0) => Some(DraftChoice::Keep),
        Some(1) => Some(DraftChoice::Discard),
        Some(_) => Some(DraftChoice::Back),
        None if shown.dismissed => Some(DraftChoice::Back),
        None => None,
    };
    if let Some(choice) = choice {
        dialog::Dialog::reset(ui.ctx(), id);
        events.push(UiEvent::RestoreDraft(choice));
    }
}

/// The compose dialog, when a draft is open.
pub fn dialog(ui: &mut Ui, state: &State, events: &mut Vec<UiEvent>) {
    let Some(compose) = &state.compose else {
        return;
    };
    let id = egui::Id::new("compose");
    let mut send = Choice::new(crate::label("send"), dialog::Role::Default);
    send.enabled = state.compose_sendable();
    let choices = vec![
        send,
        Choice::new(crate::label("cancel"), dialog::Role::Cancel),
    ];
    let send_index = 0; // choices[0] is Send; the cancel closes below.
    // The body can be resized; egui keeps the size for this id while the
    // app runs, so the window opens at the size it was last left at.
    let shown = dialog::Dialog::new(id, crate::label("compose-title"))
        .buttons(choices)
        .resizable(egui::vec2(720.0, 560.0), egui::vec2(480.0, 320.0))
        .show(ui.ctx(), |ui| {
            form(ui, compose, state, events);
        });
    if shown.chosen == Some(send_index) {
        events.push(UiEvent::SendCompose);
        dialog::Dialog::reset(ui.ctx(), id);
    } else if shown.chosen.is_some() || shown.dismissed {
        dialog::Dialog::reset(ui.ctx(), id);
        events.push(UiEvent::CloseCompose);
    }
}

/// A recipient field with its address completions under it. While the
/// list shows and the field has the keyboard, Up and Down move through
/// it, Enter or Tab take the highlighted address, and Escape closes it:
/// those keys are taken before the field sees them.
fn recipients(
    ui: &mut Ui,
    compose: &ComposeState,
    label: &str,
    field: ComposeField,
    width: f32,
    events: &mut Vec<UiEvent>,
) {
    let id = egui::Id::new("compose-recipients").with(label);
    let suggest = compose
        .suggest
        .as_ref()
        .filter(|s| s.field == field && !s.items.is_empty());
    let focused = ui.memory(|m| m.has_focus(id));
    if let Some(suggest) = suggest
        && focused
    {
        let none = egui::Modifiers::NONE;
        ui.input_mut(|i| {
            if i.consume_key(none, egui::Key::ArrowDown) {
                events.push(UiEvent::SuggestMove(1));
            }
            if i.consume_key(none, egui::Key::ArrowUp) {
                events.push(UiEvent::SuggestMove(-1));
            }
            if i.consume_key(none, egui::Key::Enter) || i.consume_key(none, egui::Key::Tab) {
                events.push(UiEvent::SuggestAccept(Some(suggest.cursor)));
            }
            if i.consume_key(none, egui::Key::Escape) {
                events.push(UiEvent::SuggestClose);
            }
        });
    }
    let mut buffer = compose.recipients(field).to_owned();
    let response = ui.add(field::text_edit(&mut buffer, &crate::label(label), width).id(id));
    if response.changed() {
        events.push(UiEvent::ComposeField(field, buffer));
    }
    if let Some(suggest) = suggest
        && (focused || response.has_focus())
    {
        dropdown(ui, &response, suggest, events);
    }
}

/// The completions under `field`: the name strong, the address weak, the
/// typed part highlighted, the keyboard's row selected.
fn dropdown(
    ui: &Ui,
    field: &egui::Response,
    suggest: &crate::state::Suggest,
    events: &mut Vec<UiEvent>,
) {
    egui::Area::new(field.id.with("suggestions"))
        .order(egui::Order::Foreground)
        .fixed_pos(field.rect.left_bottom())
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(field.rect.width());
                let strong = ui.visuals().strong_text_color();
                let weak = ui.visuals().weak_text_color();
                let mark = ui.visuals().selection.bg_fill.gamma_multiply(0.5);
                let body = egui::TextStyle::Body.resolve(ui.style());
                let bold = toolkit::fonts::bound(ui.ctx(), toolkit::fonts::heading(body.size));
                for (index, item) in suggest.items.iter().enumerate() {
                    let mut job = egui::text::LayoutJob::default();
                    let mut add = |text: &str, font: &egui::FontId, colour: egui::Color32| {
                        let base = egui::TextFormat::simple(font.clone(), colour);
                        match crate::suggest::highlight(text, &suggest.query) {
                            Some(range) => {
                                job.append(&text[..range.start], 0.0, base.clone());
                                let marked = egui::TextFormat {
                                    background: mark,
                                    ..base.clone()
                                };
                                job.append(&text[range.clone()], 0.0, marked);
                                job.append(&text[range.end..], 0.0, base);
                            }
                            None => job.append(text, 0.0, base),
                        }
                    };
                    if !item.name.is_empty() {
                        add(&item.name, &bold, strong);
                        add("  ", &body, weak);
                    }
                    add(&item.email, &body, weak);
                    let row = toolkit::selectable(ui, index == suggest.cursor, job);
                    if row.clicked() {
                        events.push(UiEvent::SuggestAccept(Some(index)));
                    }
                }
            });
        });
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
        for (label, field) in [
            ("to", ComposeField::To),
            ("cc", ComposeField::Cc),
            ("bcc", ComposeField::Bcc),
        ] {
            recipients(ui, compose, label, field, width, events);
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
        // A reply's quote can sit above or below the person's text. The
        // switch is off the table once the quote itself was edited.
        if let Some(reply) = &compose.reply {
            let movable = crate::quote::moved(
                &compose.draft.body,
                &reply.quote,
                &reply.signature,
                reply.position,
            )
            .is_some();
            let mut below = reply.position == crate::quote::Position::Below;
            let switch = Toggle::new(&mut below, crate::label("reply-below-quote"));
            if ui.add_enabled(movable, switch).changed() {
                let to = if below {
                    crate::quote::Position::Below
                } else {
                    crate::quote::Position::Above
                };
                events.push(UiEvent::ComposeReplyPosition(to));
            }
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
    }
}
