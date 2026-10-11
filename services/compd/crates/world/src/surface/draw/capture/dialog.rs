// SPDX-License-Identifier: MIT OR Apache-2.0
//! `ContinueDialog` — the 5-minute keep-alive dialog for video captures. Shows
//! a countdown; the user must click Continue within the window or the capture
//! stops. Its own small centered screen-space instance.

use crate::capture::session::message::CaptureMessage;
use crate::strings;
use ui::UiProgram;

use crate::surface::draw::capture::style;

pub struct ContinueDialog {
    /// Seconds remaining before the capture auto-stops.
    pub seconds: u32,
}

impl ContinueDialog {
    pub fn new(seconds: u32) -> Self {
        Self { seconds }
    }
}

impl UiProgram for ContinueDialog {
    type Message = CaptureMessage;

    fn update(&mut self, message: Self::Message) {
        if let CaptureMessage::SetCountdown(s) = message {
            self.seconds = s;
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, emit: &mut Vec<CaptureMessage>) {
        let accent = style::accent(ui);
        let seconds = self.seconds.to_string();
        style::centred_dialog(ui, "capture-continue", accent, |ui| {
            ui.label(egui::RichText::new(strings::text("capture-continue-title")).size(20.0));
            ui.label(
                egui::RichText::new(strings::with("capture-continue-body", &[("seconds", &seconds)]))
                    .size(14.0)
                    .color(style::weak(ui)),
            );
            ui.horizontal(|ui| {
                if style::button(ui, strings::text("capture-continue"), Some(accent)).clicked() {
                    emit.push(CaptureMessage::ContinueCapture);
                }
                let danger = style::danger(ui);
                if style::button(ui, strings::text("capture-stop-now"), Some(danger)).clicked() {
                    emit.push(CaptureMessage::StopFromDialog);
                }
            });
        });
    }
}
