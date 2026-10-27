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

use egui::Ui;
use toolkit::dialog::{self, Choice};
use toolkit::field;
use toolkit::toggle::Toggle;

use super::UiEvent;
use crate::state::{
    ComposeAttachment, ComposeAttachmentState, ComposeField, ComposeState, Confirm, DraftChoice,
    State,
};

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
    let id = egui::Id::unique(key);
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
    let id = egui::Id::unique("restore-draft");
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
pub fn body_focused(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<egui::Id>(egui::Id::unique("email.compose.body.focus")))
        .is_some_and(|id| ctx.memory(|m| m.has_focus(id)))
}

/// The compose dialog, when a draft is open.
pub fn dialog(
    ui: &mut Ui,
    state: &State,
    commands: &toolkit::Registry<State>,
    strings: &toolkit::Strings,
    palette: &editor::Palette,
    events: &mut Vec<UiEvent>,
) {
    let Some(compose) = &state.compose else {
        return;
    };
    let id = egui::Id::unique("compose");
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
        .enabled(state.attachment_chooser.is_none() && !state.documents.modal())
        .buttons(choices)
        .resizable(egui::vec2(720.0, 560.0), egui::vec2(480.0, 320.0))
        .show(ui.ctx(), |ui| {
            events.extend(
                commands
                    .menu_bar_for(
                        ui,
                        state,
                        strings,
                        &["compose-file-menu", "compose-attachments-menu"],
                    )
                    .into_iter()
                    .map(|(id, origin)| UiEvent::Command(id, origin)),
            );
            form(ui, compose, state, palette, events);
        });
    if shown.chosen == Some(send_index) {
        events.push(UiEvent::SendCompose);
        dialog::Dialog::reset(ui.ctx(), id);
    } else if shown.chosen.is_some() || shown.dismissed {
        dialog::Dialog::reset(ui.ctx(), id);
        events.push(UiEvent::CloseCompose);
    }
    if state.documents.replace.is_some() {
        let id = egui::Id::unique("compose-document-replace");
        let shown = dialog::Dialog::new(id, crate::label("document-replace-title"))
            .buttons(vec![
                Choice::new(crate::label("document-replace"), dialog::Role::Default),
                Choice::new(crate::label("cancel"), dialog::Role::Cancel),
            ])
            .show(ui.ctx(), |ui| {
                ui.label(crate::label("document-replace-body"));
            });
        if shown.chosen.is_some() || shown.dismissed {
            dialog::Dialog::reset(ui.ctx(), id);
            events.push(UiEvent::DocumentReplace(shown.chosen == Some(0)));
        }
    }
}

/// A recipient field with its address completions under it. While the
/// list shows and the field has the keyboard, Up and Down move through
/// it, Enter or Tab take the highlighted address, and Escape closes it:
/// those keys are taken before the field sees them.
fn recipients(
    ui: &mut Ui,
    compose: &ComposeState,
    field: ComposeField,
    label: egui::Id,
    width: f32,
    events: &mut Vec<UiEvent>,
) {
    let id = egui::Id::unique("compose-recipients").with(row_label(field));
    let suggest = compose
        .suggest
        .as_ref()
        .filter(|s| ui.is_enabled() && s.field == field && !s.items.is_empty());
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
    let response = ui
        .add(field::text_edit(&mut buffer, "", width).id(id))
        .labelled_by(label);
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

/// The space between a row's label and its field, and between a field and
/// the buttons after it.
const GAP: f32 = 8.0;

/// The space between the To row's Cc, Bcc and Reply-To buttons.
const BUTTON_GAP: f32 = 2.0;

/// The catalogue key of a row's label.
fn row_label(field: ComposeField) -> &'static str {
    match field {
        ComposeField::To => "to",
        ComposeField::Cc => "cc",
        ComposeField::Bcc => "bcc",
        ComposeField::ReplyTo => "reply-to",
        ComposeField::Subject => "subject",
        ComposeField::Body => "body",
    }
}

/// The To row's button for an optional row: its icon and hover text.
fn row_button(field: ComposeField) -> (toolkit::Icon, &'static str) {
    match field {
        ComposeField::Cc => (toolkit::Icon::Copy, "show-cc"),
        ComposeField::Bcc => (toolkit::Icon::EyeOff, "show-bcc"),
        _ => (toolkit::Icon::Reply, "show-reply-to"),
    }
}

/// One form row as Thunderbird lays it out: the label outside the field,
/// right-aligned in a column `column` wide, then `add` with the rest of the
/// row and the label's id (for the field's accessible name).
fn row<R>(ui: &mut Ui, text: &str, column: f32, add: impl FnOnce(&mut Ui, egui::Id) -> R) -> R {
    row_of(ui, text, column, field::height_of(ui.ctx()), add)
}

/// [`row`] for a row `height` tall (the From row holds a push button,
/// taller than a field): the label centres on the row's middle.
fn row_of<R>(
    ui: &mut Ui,
    text: &str,
    column: f32,
    height: f32,
    add: impl FnOnce(&mut Ui, egui::Id) -> R,
) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let label = ui
            .allocate_ui_with_layout(
                egui::vec2(column, height),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.set_min_width(column);
                    ui.label(text)
                },
            )
            .inner;
        add(ui, label.id)
    })
    .inner
}

/// One removable file chip. It draws only the state's last inspection.
fn attachment_chip(
    ui: &mut Ui,
    attachment: &ComposeAttachment,
    index: usize,
    icon: f32,
    events: &mut Vec<UiEvent>,
) {
    let size = attachment
        .size
        .map(crate::attachments::human_size)
        .unwrap_or_else(|| crate::label("compose-attachment-size-unknown"));
    let text = crate::label_with(
        "compose-attachment-chip",
        &[("name", &attachment.label), ("size", &size)],
    );
    let bad = attachment.state != ComposeAttachmentState::Ready;
    let text = if bad {
        let status = crate::label(attachment.state.label());
        crate::label_with(
            "compose-attachment-status",
            &[("file", &text), ("state", &status)],
        )
    } else {
        text
    };
    // Decide wrapping before Frame::group consumes the row remainder. A
    // long name gets the full row's label budget, rather than shrinking
    // into the space left after the preceding chip.
    let frame = egui::Frame::group(ui.style());
    let margin = frame.total_margin().sum().x;
    let spacing = ui.spacing().item_spacing.x;
    let font = egui::TextStyle::Body.resolve(ui.style());
    let minimum_label_width = font.size * 4.0;
    let natural = ui
        .painter()
        .layout_no_wrap(text.clone(), font, ui.visuals().text_color())
        .size();
    let full_width = ui.max_rect().width();
    let width = natural
        .x
        .min((full_width - icon - spacing - margin).max(0.0))
        .max(minimum_label_width);
    let outer_width = width + icon + spacing + margin;
    // available_width() is the full row width in a wrapping layout. Scopes
    // advance their already drawn rect without wrapping it, so wrap before
    // push_id/Frame using the space left on this row instead.
    let remaining_width = ui.available_size_before_wrap().x;
    if outer_width > remaining_width && remaining_width < full_width {
        ui.end_row();
    }
    let height = icon.max(natural.y);
    ui.push_id((index, &attachment.path), |ui| {
        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                let mut text = egui::RichText::new(text);
                if bad {
                    text = text.color(ui.visuals().error_fg_color);
                }
                ui.add_sized([width, height], egui::Label::new(text).truncate())
                    .on_hover_text(attachment.path.display().to_string());
                let remove = toolkit::button::IconButton::new(toolkit::Icon::X)
                    .size(icon)
                    .tooltip(crate::label_with(
                        "compose-attachment-remove",
                        &[("name", &attachment.label)],
                    ));
                if ui.add(remove).clicked() {
                    events.push(UiEvent::ComposeAttachmentRemove(attachment.path.clone()));
                }
            });
        });
    });
}

/// The dialog's fields. Each edit is one event; the state owns the draft.
///
/// The rows, labels outside on the left: From (the identity, with
/// Identities… at the right), To (with the Cc, Bcc and Reply-To buttons at
/// the right), the optional rows the person opened, and Subject.
fn form(
    ui: &mut Ui,
    compose: &ComposeState,
    state: &State,
    palette: &editor::Palette,
    events: &mut Vec<UiEvent>,
) {
    // Capture the Resize body's content width before any row can expand its
    // bounds. The attachment column uses the same budget as Subject.
    let form_width = ui.available_width();
    // Leave room below the editor. Otherwise a fill-height editor makes
    // the resizable dialog grow again on every frame.
    let spacing = ui.spacing().item_spacing.y;
    let mut footer = if compose.reply.is_some() {
        ui.spacing()
            .interact_size
            .y
            .max(toolkit::toggle::track_of(ui.ctx()).y)
            + spacing
    } else {
        0.0
    };
    let text_height = |text: String| {
        ui.painter()
            .layout(
                text,
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().text_color(),
                ui.available_width(),
            )
            .size()
            .y
            + spacing
    };
    if let Some(from) = &compose.stale {
        footer += text_height(crate::label_with(
            "compose-from-new",
            &[("old", &compose.draft.from), ("new", &from.address)],
        )) + toolkit::chrome::Chrome::of(ui.ctx()).metrics.push_height
            + spacing;
    }
    if let Some(error) = &compose.error {
        footer += text_height(crate::label_with("send-failed", &[("error", error)]));
    }
    if compose.busy {
        footer += text_height(crate::label("sending"));
    }
    // The label column fits the widest label any row can have, so opening
    // a row never moves the fields sideways.
    let font = egui::TextStyle::Body.resolve(ui.style());
    let column = [ComposeField::To]
        .into_iter()
        .chain(ComposeField::OPTIONAL)
        .chain([ComposeField::Subject])
        .map(|f| crate::label(row_label(f)))
        .chain([crate::label("from"), crate::label("compose-attachments")])
        .map(|text| {
            ui.painter()
                .layout_no_wrap(text, font.clone(), egui::Color32::PLACEHOLDER)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
        .ceil();
    let attachment_width = (form_width - column - GAP).max(0.0);
    let icon = field::height_of(ui.ctx());
    // While a send is in flight every field is read-only: the dialog shows
    // exactly what is being sent.
    ui.add_enabled_ui(!compose.busy, |ui| {
        // From: every identity of the accounts that can send; the draft's
        // own identity is the one shown. Identities… edits the draft's
        // account's identities in place.
        let senders: Vec<(String, settings::email::Identity)> = state
            .accounts
            .iter()
            .filter(|v| v.entry.enabled)
            .flat_map(|v| {
                v.entry
                    .senders()
                    .into_iter()
                    .map(|identity| (v.entry.id.clone(), identity))
            })
            .collect();
        let height = toolkit::chrome::Chrome::of(ui.ctx())
            .metrics
            .push_height
            .max(field::height_of(ui.ctx()));
        row_of(ui, &crate::label("from"), column, height, |ui, label| {
            let identities =
                toolkit::button::PushButton::secondary(crate::label("identities-open"));
            // The button is laid out first from the right, so the identity
            // takes exactly what is left.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(identities).clicked() {
                    events.push(UiEvent::OpenIdentities(compose.account.clone()));
                }
                let width = ui.available_width();
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    // A long "Name <address>" is cut short, never widening
                    // the dialog.
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    ui.set_max_width(width);
                    if senders.len() > 1 {
                        let names: Vec<String> = senders
                            .iter()
                            .map(|(_, i)| format!("{} <{}>", i.name, i.address))
                            .collect();
                        let mut at = senders
                            .iter()
                            .position(|(account, i)| {
                                *account == compose.account && i.id == compose.identity
                            })
                            .or_else(|| {
                                senders
                                    .iter()
                                    .position(|(account, _)| *account == compose.account)
                            })
                            .unwrap_or(0);
                        let combo =
                            toolkit::combo::show(ui, "compose-from", &mut at, &names, Some(width))
                                .labelled_by(label);
                        if combo.changed() {
                            let (account, identity) = &senders[at];
                            events.push(UiEvent::ComposeIdentity {
                                account: account.clone(),
                                identity: identity.id.clone(),
                            });
                        }
                    } else {
                        // One identity: what the message goes out as.
                        let from = if compose.draft.from_name.is_empty() {
                            compose.draft.from.clone()
                        } else {
                            format!("{} <{}>", compose.draft.from_name, compose.draft.from)
                        };
                        ui.label(from).labelled_by(label);
                    }
                });
            });
        });
        // To, its optional-row buttons at the right.
        row(ui, &crate::label("to"), column, |ui, label| {
            // The buttons are laid out first from the right (so they end
            // where Identities… ends), then the field takes the rest.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = BUTTON_GAP;
                for extra in ComposeField::OPTIONAL.into_iter().rev() {
                    let (glyph, tip) = row_button(extra);
                    let button = toolkit::button::IconButton::new(glyph)
                        .size(icon)
                        .selected(compose.shows(extra))
                        .tooltip(crate::label(tip));
                    if ui.add(button).clicked() {
                        events.push(UiEvent::ComposeShow(extra));
                    }
                }
                ui.add_space(GAP - BUTTON_GAP);
                let width = ui.available_width();
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_max_width(width);
                    recipients(ui, compose, ComposeField::To, label, width, events);
                });
            });
        });
        for extra in ComposeField::OPTIONAL {
            if compose.shows(extra) {
                row(ui, &crate::label(row_label(extra)), column, |ui, label| {
                    let width = ui.available_width();
                    recipients(ui, compose, extra, label, width, events);
                });
            }
        }
        row(ui, &crate::label("subject"), column, |ui, label| {
            let mut buffer = compose.draft.subject.clone();
            let width = ui.available_width();
            let response = ui
                .add(
                    field::text_edit(&mut buffer, "", width)
                        .id(egui::Id::unique("compose-subject")),
                )
                .labelled_by(label);
            if response.changed() {
                events.push(UiEvent::ComposeField(ComposeField::Subject, buffer));
            }
        });
        row(ui, &crate::label("compose-attachments"), column, |ui, _| {
            ui.vertical(|ui| {
                ui.set_max_width(attachment_width);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add(
                            toolkit::button::IconButton::new(toolkit::Icon::Paperclip)
                                .size(icon)
                                .tooltip(crate::label("compose-attach")),
                        )
                        .clicked()
                    {
                        events.push(UiEvent::Command(
                            "compose.attach",
                            toolkit::action::Origin::Menu,
                        ));
                    }
                    for (index, attachment) in compose.attachments.iter().enumerate() {
                        attachment_chip(ui, attachment, index, icon, events);
                    }
                });
                if !compose.attachments.is_empty() {
                    ui.weak(crate::label_with(
                        "compose-attachments-total",
                        &[(
                            "size",
                            &crate::attachments::human_size(compose.attachment_size()),
                        )],
                    ));
                }
                if let Some(error) = &state.attachment_notice {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if let Some(error) = &compose.attachment_error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            });
        });
        ui.add_space(4.0);
        // The text takes the composer's own padding, font and size.
        toolkit::content::surface(ui, &state.settings.composer, |ui| {
            let view = editor::View {
                read_only: compose.busy,
                label: Some(crate::label("body")),
                placeholder: Some(crate::label("body")),
                font_size: Some(egui::TextStyle::Body.resolve(ui.style()).size),
                ..editor::View::prose()
            };
            let row_height = (view.font_size.unwrap() * view.line_height).round();
            let height = (row_height * 8.0).max(ui.available_height() - footer);
            ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                let output = editor::show(
                    ui,
                    ("email.compose.body", &compose.draft_id),
                    &compose.body.doc.doc(),
                    palette,
                    &view,
                );
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::unique("email.compose.body.focus"),
                        output.response.id,
                    )
                });
                events.extend(output.events.into_iter().map(UiEvent::ComposeEdit));
            });
        });
        // A reply's quote can sit above or below the person's text. The
        // switch is off the table once the quote itself was edited.
        if let Some(reply) = &compose.reply {
            let movable = compose.reply_movable();
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn small_remove_icon_does_not_clip_the_text_line() {
        let attachment = ComposeAttachment {
            path: "/home/user/note.txt".into(),
            name: "note.txt".into(),
            label: "note.txt".into(),
            size: Some(5),
            state: ComposeAttachmentState::Ready,
        };
        let mut h = Harness::builder().build_ui(move |ui| {
            ui.label("Reference line");
            ui.horizontal_wrapped(|ui| {
                attachment_chip(ui, &attachment, 0, 2.0, &mut Vec::new());
            });
        });
        toolkit::install(
            &h.ctx,
            &toolkit::Theme::for_context(design::DesignContext::default()),
        );
        h.run();
        let text = crate::label_with(
            "compose-attachment-chip",
            &[("name", "note.txt"), ("size", "5 B")],
        );
        assert!(
            h.get_by_label(&text).rect().height()
                >= h.get_by_label("Reference line").rect().height()
        );
    }

    #[test]
    fn narrow_chip_keeps_four_em_of_its_name_visible() {
        let attachment = ComposeAttachment {
            path: "/home/user/quarterly-report.txt".into(),
            name: "quarterly-report.txt".into(),
            label: "quarterly-report.txt".into(),
            size: Some(5),
            state: ComposeAttachmentState::Ready,
        };
        let mut h = Harness::builder()
            .with_size(egui::vec2(240.0, 120.0))
            .build_ui(move |ui| {
                ui.set_max_width(40.0);
                ui.horizontal_wrapped(|ui| {
                    let icon = field::height_of(ui.ctx());
                    attachment_chip(ui, &attachment, 0, icon, &mut Vec::new());
                });
            });
        toolkit::install(
            &h.ctx,
            &toolkit::Theme::for_context(design::DesignContext::default()),
        );
        h.run();
        let text = crate::label_with(
            "compose-attachment-chip",
            &[("name", "quarterly-report.txt"), ("size", "5 B")],
        );
        let rect = h.get_by_label(&text).rect();
        let minimum = egui::TextStyle::Body.resolve(&h.ctx.global_style()).size * 4.0;
        assert!(
            rect.width() >= minimum - 0.5,
            "{rect:?} must retain about {minimum} points of the filename"
        );
        assert!(rect.intersects(h.ctx.content_rect()));
    }

    #[test]
    fn attachment_catalogue_keys_resolve() {
        for key in [
            "compose-attachments",
            "compose-attachment-chip",
            "compose-attachment-status",
            "compose-attachment-name",
            "compose-attachment-remove",
            "compose-attachment-ready",
            "compose-attachment-missing",
            "compose-attachment-not-file",
            "compose-attachment-unreadable",
            "compose-attachment-relative",
            "compose-attachment-size-unknown",
            "compose-attachments-total",
            "compose-attachments-fix",
            "compose-attachments-too-large",
            "compose-attachments-too-large-default",
            "compose-not-open",
            "no-such-attachment",
            "sending",
        ] {
            let text = crate::label_with(
                key,
                &[
                    ("name", "note.txt"),
                    ("size", "5 B"),
                    ("state", "Missing"),
                    ("file", "note.txt"),
                    ("directory", "reports"),
                    ("limit", "25 MB"),
                ],
            );
            assert_ne!(text, key, "missing Fluent key {key}");
            assert!(!text.is_empty());
        }
    }
}
