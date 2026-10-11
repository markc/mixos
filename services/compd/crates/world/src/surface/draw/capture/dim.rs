// SPDX-License-Identifier: MIT OR Apache-2.0
//! `RegionDim` — the dark backdrop with a clear hole over the captured region.
//! Drawn BELOW windows (its own `CAPTURE_DIM` layer) so windows in the region
//! stay bright while everything else is dimmed. Click-through.

use crate::capture::session::message::{CaptureMessage, OverlayRect};
use ui::UiProgram;

use crate::surface::draw::capture::{mask, style};

pub struct RegionDim {
    pub screen_w: i32,
    pub screen_h: i32,
    pub rect: OverlayRect,
}

impl RegionDim {
    pub fn new(screen_w: i32, screen_h: i32, rect: OverlayRect) -> Self {
        Self {
            screen_w,
            screen_h,
            rect,
        }
    }
}

impl UiProgram for RegionDim {
    type Message = CaptureMessage;

    fn update(&mut self, message: Self::Message) {
        if let CaptureMessage::SetRegion(r) = message {
            self.rect = r;
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, _emit: &mut Vec<CaptureMessage>) {
        mask::mask_with_hole(
            ui.painter(),
            self.screen_w,
            self.screen_h,
            Some(self.rect),
            style::DIM_MASK,
            None,
        );
    }
}
