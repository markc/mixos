// SPDX-License-Identifier: MIT OR Apache-2.0
//! The notification pill: one line of text on a dark rounded pill, centred in
//! its surface.
//!
//! Deliberately inert — no buttons, no input. The surface lives in the
//! notification presenter's own registry, which no world hit-tests, so it is
//! click-through by construction; anything interactive here would be a target
//! the user cannot reach. The only state is the message.
//!
//! The ANIMATION is not here. Sliding the pill up from the bottom edge is done
//! by moving the whole surface (`registry.set_location`) once per frame, one
//! integer write, where animating inside the UI would re-render the text every
//! frame for the same result.
use ui::UiProgram;

/// Compositor → surface. One variant: the text to show.
#[derive(Clone, Debug)]
pub enum NotifyMessage {
    SetText(String),
}

#[derive(Default)]
pub struct NotifyUi {
    pub message: String,
}

impl NotifyUi {
    pub fn new(message: String) -> Self {
        Self { message }
    }
}

impl UiProgram for NotifyUi {
    type Message = NotifyMessage;

    fn update(&mut self, message: Self::Message) {
        match message {
            NotifyMessage::SetText(t) => self.message = t,
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, _emit: &mut Vec<NotifyMessage>) {
        // A dark rounded pill, centred: notifications read over any desktop,
        // so this keeps its own contrast rather than the theme's.
        let text = egui::RichText::new(self.message.clone())
            .size(14.0)
            .color(egui::Color32::from_rgba_unmultiplied(240, 245, 255, 245));
        egui::Area::new(egui::Id::new("notify-pill"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgba_unmultiplied(15, 18, 26, 224))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(26)))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::symmetric(18, 10))
                    .show(ui, |ui| {
                        ui.label(text);
                    });
            });
    }
}
