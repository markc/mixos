// SPDX-License-Identifier: MIT OR Apache-2.0
//! `RegionBorder` — the bright outline around the captured region. Drawn above
//! everything (screen-space, top layer). Fully transparent except the outline,
//! and click-through (the interface adds the passthrough layer bit).

use crate::capture::session::message::{CaptureMessage, OverlayRect};
use ui::UiProgram;

use crate::surface::draw::capture::{mask, style};

pub struct RegionBorder {
    pub screen_w: i32,
    pub screen_h: i32,
    pub rect: OverlayRect,
}

impl RegionBorder {
    pub fn new(screen_w: i32, screen_h: i32, rect: OverlayRect) -> Self {
        Self {
            screen_w,
            screen_h,
            rect,
        }
    }
}

impl UiProgram for RegionBorder {
    type Message = CaptureMessage;

    fn update(&mut self, message: Self::Message) {
        if let CaptureMessage::SetRegion(r) = message {
            self.rect = r;
        }
    }

    fn setup(&mut self, ctx: &egui::Context) {
        style::install(ctx);
    }

    fn show(&mut self, ui: &mut egui::Ui, _emit: &mut Vec<CaptureMessage>) {
        style::keep_up(ui.ctx());
        // Inflated by the border width, so the stroke lands entirely OUTSIDE
        // the captured region (otherwise its pixels show up in the capture).
        let bw = style::BORDER_WIDTH as i32;
        let outer = OverlayRect {
            x: self.rect.x - bw,
            y: self.rect.y - bw,
            w: self.rect.w + 2 * bw,
            h: self.rect.h + 2 * bw,
        };
        let painter = ui.painter();
        mask::region_border(
            painter,
            self.screen_w,
            self.screen_h,
            outer,
            style::accent(ui),
            bw,
        );
    }
}
