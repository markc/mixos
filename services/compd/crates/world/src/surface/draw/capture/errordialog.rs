// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `ErrorDialog` — a small centered modal shown when a capture can't proceed
//! (e.g. no working hardware video encoder). A message + a single OK button that
//! emits [`CaptureMessage::Discard`] (the interface tears the capture down).

use crate::capture::session::message::CaptureMessage;
use crate::strings;
use ui::UiProgram;

use crate::surface::draw::capture::style;

pub struct ErrorDialog {
    message: String,
}

impl ErrorDialog {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl UiProgram for ErrorDialog {
    type Message = CaptureMessage;

    fn update(&mut self, _message: Self::Message) {}

    fn setup(&mut self, ctx: &egui::Context) {
        style::install(ctx);
    }

    fn show(&mut self, ui: &mut egui::Ui, emit: &mut Vec<CaptureMessage>) {
        style::keep_up(ui.ctx());
        let danger = style::danger(ui);
        let message = self.message.clone();
        style::centred_dialog(ui, "capture-error", danger, |ui| {
            ui.label(
                egui::RichText::new(strings::text("capture-error-title"))
                    .size(20.0)
                    .color(danger),
            );
            ui.label(
                egui::RichText::new(message)
                    .size(14.0)
                    .color(style::weak(ui)),
            );
            let accent = style::accent(ui);
            if style::button(ui, strings::text("capture-ok"), Some(accent)).clicked() {
                emit.push(CaptureMessage::Discard);
            }
        });
    }
}
