// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `EncodingDialog` — shown while the optimized (software) re-encode runs in the
//! manual "Optimized encoding" path. A title + a progress bar updated each frame
//! by the interface via [`CaptureMessage::SetEncodeProgress`]. Own small centered
//! screen-space instance (replaces the save dialog during encoding).

use crate::capture::session::message::CaptureMessage;
use crate::strings;
use ui::UiProgram;

use crate::surface::draw::capture::style;

pub struct EncodingDialog {
    /// Progress percent (0..=100).
    pub percent: u32,
}

impl EncodingDialog {
    pub fn new() -> Self {
        Self { percent: 0 }
    }
}

impl Default for EncodingDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl UiProgram for EncodingDialog {
    type Message = CaptureMessage;

    fn update(&mut self, message: Self::Message) {
        if let CaptureMessage::SetEncodeProgress(p) = message {
            self.percent = p.min(100);
        }
    }

    fn setup(&mut self, ctx: &egui::Context) {
        style::install(ctx);
    }

    fn show(&mut self, ui: &mut egui::Ui, _emit: &mut Vec<CaptureMessage>) {
        style::keep_up(ui.ctx());
        let accent = style::accent(ui);
        let percent = self.percent;
        style::centred_dialog(ui, "capture-encoding", accent, |ui| {
            ui.label(egui::RichText::new(strings::text("capture-encoding-title")).size(20.0));
            ui.label(
                egui::RichText::new(strings::with(
                    "capture-encoding-body",
                    &[("percent", &percent.to_string())],
                ))
                .size(14.0)
                .color(style::weak(ui)),
            );
            ui.add(
                egui::ProgressBar::new(percent as f32 / 100.0)
                    .desired_width(320.0)
                    .desired_height(10.0)
                    .fill(accent),
            );
        });
    }
}
