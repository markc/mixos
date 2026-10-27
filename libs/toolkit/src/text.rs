// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Labels whose colour is resolved when they are drawn.

pub struct Text {
    text: String,
    role: &'static str,
}

/// A size as people read it: `512 B`, `1.2 KB`, `3.4 MB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if size < 10.0 {
        format!("{size:.1} {}", UNITS[unit])
    } else {
        format!("{size:.0} {}", UNITS[unit])
    }
}

pub fn dim(text: impl Into<String>) -> Text {
    role(text, "text_dim")
}
pub fn faint(text: impl Into<String>) -> Text {
    role(text, "text_faint")
}
pub fn role(text: impl Into<String>, role: &'static str) -> Text {
    Text {
        text: text.into(),
        role,
    }
}

impl egui::Widget for Text {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let palette = crate::chrome::Chrome::of(ui.ctx()).palette;
        let colour = palette.get(self.role).expect("a registered chrome role");
        ui.label(egui::RichText::new(self.text).color(colour))
    }
}
