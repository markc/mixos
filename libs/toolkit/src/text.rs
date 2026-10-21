// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Labels whose colour is resolved when they are drawn.

pub struct Text {
    text: String,
    role: &'static str,
}

pub fn dim(text: impl Into<String>) -> Text { role(text, "text_dim") }
pub fn faint(text: impl Into<String>) -> Text { role(text, "text_faint") }
pub fn role(text: impl Into<String>, role: &'static str) -> Text {
    Text { text: text.into(), role }
}

impl egui::Widget for Text {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let palette = crate::chrome::Chrome::of(ui.ctx()).palette;
        let colour = palette.get(self.role).expect("a registered chrome role");
        ui.label(egui::RichText::new(self.text).color(colour))
    }
}
