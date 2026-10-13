// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A full-screen mask with an optional rectangular hole, and a region
//! outline, painted straight onto the overlay's painter.
//!
//! Coordinates are the overlay's own logical units (egui points). The hole is
//! clipped to the screen by its edges: clamping `x` alone, without shrinking
//! the width, would shift a hole that hangs off the left edge to x = 0 at full
//! width, which is the wrong place.

use egui::{Color32, Painter, Rect, Stroke, StrokeKind, pos2};

use crate::capture::session::message::OverlayRect;

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect::from_min_size(
        pos2(x as f32, y as f32),
        egui::vec2(w.max(0) as f32, h.max(0) as f32),
    )
}

/// Paint a `screen_w` × `screen_h` mask of `mask`. With `hole = Some(r)` the
/// rect is left clear, outlined by `border` when given.
pub fn mask_with_hole(
    painter: &Painter,
    screen_w: i32,
    screen_h: i32,
    hole: Option<OverlayRect>,
    mask: Color32,
    border: Option<(Color32, f32)>,
) {
    let Some(h) = hole else {
        painter.rect_filled(rect(0, 0, screen_w, screen_h), 0.0, mask);
        return;
    };
    let left = h.x.clamp(0, screen_w);
    let right = (h.x + h.w).clamp(0, screen_w);
    let top = h.y.clamp(0, screen_h);
    let bottom = (h.y + h.h).clamp(0, screen_h);
    // Four bands around the hole: above, below, left of and right of it.
    painter.rect_filled(rect(0, 0, screen_w, top), 0.0, mask);
    painter.rect_filled(rect(0, bottom, screen_w, screen_h - bottom), 0.0, mask);
    painter.rect_filled(rect(0, top, left, bottom - top), 0.0, mask);
    painter.rect_filled(rect(right, top, screen_w - right, bottom - top), 0.0, mask);
    if let Some((colour, width)) = border
        && right > left
        && bottom > top
    {
        painter.rect_stroke(
            rect(left, top, right - left, bottom - top),
            0.0,
            Stroke::new(width, colour),
            StrokeKind::Inside,
        );
    }
}

/// A `bw`-thick outline of `region`, as four independent edges. An edge is
/// drawn only when its own line is on screen and is clipped to the screen, so
/// a region panned off any side loses that side's edge while the
/// perpendicular edges keep their true positions.
pub fn region_border(
    painter: &Painter,
    screen_w: i32,
    screen_h: i32,
    region: OverlayRect,
    colour: Color32,
    bw: i32,
) {
    let left = region.x;
    let top = region.y;
    let right = region.x + region.w;
    let bottom = region.y + region.h;
    let cx0 = left.clamp(0, screen_w);
    let cx1 = right.clamp(0, screen_w);
    let cy0 = top.clamp(0, screen_h);
    let cy1 = bottom.clamp(0, screen_h);
    if cx1 <= cx0 || cy1 <= cy0 {
        return;
    }
    if (0..=screen_h).contains(&top) {
        painter.rect_filled(rect(cx0, cy0, cx1 - cx0, bw), 0.0, colour);
    }
    if (0..=screen_h).contains(&bottom) {
        painter.rect_filled(rect(cx0, cy1 - bw, cx1 - cx0, bw), 0.0, colour);
    }
    if (0..=screen_w).contains(&left) {
        painter.rect_filled(rect(cx0, cy0, bw, cy1 - cy0), 0.0, colour);
    }
    if (0..=screen_w).contains(&right) {
        painter.rect_filled(rect(cx1 - bw, cy0, bw, cy1 - cy0), 0.0, colour);
    }
}
