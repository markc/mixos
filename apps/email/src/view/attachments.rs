// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The attachments popup over the reader: one row per attachment of the
//! open email (its type's icon, the name elided with the whole name on
//! hover, the size, and Save and Open), and Save All at the bottom. A part
//! the server could not decode is greyed with a note; a program or script
//! can be saved but not opened.

use egui::{RichText, Ui};
use toolkit::button::Link;
use toolkit::dialog::{self, Choice};
use toolkit::{Icon, icons};

use super::{ICON, UiEvent};
use crate::attachments;
use crate::state::State;

/// The widest a name is drawn before it is elided.
const NAME_WIDTH: f32 = 280.0;

pub fn dialog(ui: &mut Ui, state: &State, stroke: f32, events: &mut Vec<UiEvent>) {
    let email = state.current().and_then(|v| v.mail.open.as_ref());
    let list = email.map(attachments::of).unwrap_or(&[]);
    let id = egui::Id::new("dialog").with("attachments");
    let any = list.iter().any(jmap::Attachment::downloadable);
    let shown = dialog::Dialog::new(id, crate::label("attachments-title"))
        .buttons(vec![
            Choice::new(crate::label("save-all"), dialog::Role::Other).enabled(any),
            Choice::new(crate::label("done"), dialog::Role::Default),
        ])
        .show(ui.ctx(), |ui| {
            if list.is_empty() {
                ui.weak(crate::label("no-attachments"));
                return;
            }
            egui::Grid::new("attachments")
                .num_columns(4)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for (n, attachment) in list.iter().enumerate() {
                        row(ui, state, attachment, n + 1, stroke, events);
                        ui.end_row();
                    }
                });
        });
    match shown.chosen {
        Some(0) => events.push(UiEvent::SaveAllAttachments),
        Some(_) => {
            dialog::Dialog::reset(ui.ctx(), id);
            events.push(UiEvent::CloseDialog);
        }
        None if shown.dismissed => {
            dialog::Dialog::reset(ui.ctx(), id);
            events.push(UiEvent::CloseDialog);
        }
        None => {}
    }
}

fn row(
    ui: &mut Ui,
    state: &State,
    attachment: &jmap::Attachment,
    n: usize,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    let mime = attachment.mime();
    let name = attachments::file_name(attachment.name.as_deref(), n, None);
    let usable = attachment.downloadable();
    let visuals = ui.visuals();
    let tint = if usable {
        visuals.text_color()
    } else {
        visuals.weak_text_color()
    };
    ui.add(icons::image(
        attachments::icon(&mime, attachment.name.as_deref()),
        stroke,
        ICON,
        tint,
    ));
    ui.vertical(|ui| {
        ui.set_max_width(NAME_WIDTH);
        let text = if usable {
            RichText::new(&name)
        } else {
            RichText::new(&name).weak()
        };
        let mut hover = name.clone();
        if !mime.is_empty() {
            hover.push_str(&format!("\n{mime}"));
        }
        ui.add(egui::Label::new(text).truncate())
            .on_hover_text(hover);
        if !usable {
            ui.small(RichText::new(crate::label("attachment-undecodable")).weak());
        }
    });
    ui.weak(attachments::human_size(attachment.size));
    let part = attachment.part_id.clone().unwrap_or_default();
    ui.horizontal(|ui| {
        // Under way: no second download of the same part. (Text, not a
        // spinner: the window repaints only when something happens.)
        if state.attaching.values().any(|p| *p == part) {
            ui.weak(crate::label("loading"));
            return;
        }
        let save = ui.add_enabled(usable, Link::new(Icon::Save, crate::label("save")));
        if save.clicked() {
            events.push(UiEvent::Attachment(part.clone(), false));
        }
        let program = attachments::refuses_open(&mime, attachment.name.as_deref());
        let open = ui
            .add_enabled(
                usable && !program,
                Link::new(Icon::ExternalLink, crate::label("open")),
            )
            .on_disabled_hover_text(crate::label(if program {
                "attachment-save-only"
            } else {
                "attachment-undecodable"
            }));
        if open.clicked() {
            events.push(UiEvent::Attachment(part, true));
        }
    });
}
