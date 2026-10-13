// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `SaveDialog` — shown when a capture stops: Save / Save As / Discard, with an
//! optional "Optimized encoding" checkbox (video only; hidden when the
//! `background_encoder` setting makes the optimized re-encode automatic).

use crate::capture::session::message::CaptureMessage;
use crate::strings;
use ui::UiProgram;

use crate::surface::draw::capture::style;

pub struct SaveDialog {
    /// What was captured: "Screenshot" or "Video" (the interface's names).
    pub kind_label: &'static str,
    /// Whether to offer the "Optimized encoding" checkbox (video + manual mode).
    show_optimize: bool,
    /// Current checkbox state (read by the interface on Save).
    optimized: bool,
}

impl SaveDialog {
    pub fn new(kind_label: &'static str, show_optimize: bool) -> Self {
        Self {
            kind_label,
            show_optimize,
            optimized: false,
        }
    }

    /// Whether the user ticked "Optimized encoding".
    pub fn optimized(&self) -> bool {
        self.optimized
    }

    fn is_video(&self) -> bool {
        self.kind_label == "Video"
    }
}

impl UiProgram for SaveDialog {
    type Message = CaptureMessage;

    fn update(&mut self, message: Self::Message) {
        if let CaptureMessage::ToggleOptimized(v) = message {
            self.optimized = v;
        }
    }

    fn setup(&mut self, ctx: &egui::Context) {
        style::install(ctx);
    }

    fn show(&mut self, ui: &mut egui::Ui, emit: &mut Vec<CaptureMessage>) {
        style::keep_up(ui.ctx());
        let accent = style::accent(ui);
        let title = if self.is_video() {
            "capture-saved-video"
        } else {
            "capture-saved-screenshot"
        };
        // The optimized-encode checkbox only applies to video, and only when
        // the background re-encode isn't already automatic.
        let offer_optimize = self.show_optimize && self.is_video();
        let mut optimized = self.optimized;
        style::centred_dialog(ui, "capture-save", accent, |ui| {
            ui.label(egui::RichText::new(strings::text(title)).size(20.0));
            ui.label(
                egui::RichText::new(strings::text("capture-save-hint"))
                    .size(13.0)
                    .color(style::weak(ui)),
            );
            if offer_optimize
                && ui
                    .checkbox(&mut optimized, strings::text("capture-optimized"))
                    .changed()
            {
                emit.push(CaptureMessage::ToggleOptimized(optimized));
            }
            ui.horizontal(|ui| {
                if style::button(ui, strings::text("capture-save"), Some(accent)).clicked() {
                    emit.push(CaptureMessage::SaveDefault);
                }
                if style::button(ui, strings::text("capture-save-as"), None).clicked() {
                    emit.push(CaptureMessage::SaveAs);
                }
                let danger = style::danger(ui);
                if style::button(ui, strings::text("capture-discard"), Some(danger)).clicked() {
                    emit.push(CaptureMessage::Discard);
                }
            });
        });
    }
}

/// Read back by the capture driver when the dialog is dismissed. One pass of
/// staleness is invisible here: the value only changes on a click, which has
/// been through a full pass by the time it is read.
impl ui::UiSnapshot for SaveDialog {
    type Snapshot = bool;
    fn snapshot(&self) -> bool {
        self.optimized
    }
}
