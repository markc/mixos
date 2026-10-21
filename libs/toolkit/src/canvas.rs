// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The document canvas's surround and scrollbars (chrome specification
//! §3.19, §3.22).
//!
//! **Surround.** `canvas` behind the document, with a repeating dot grid in
//! `canvas_dot`; Classic draws no dots.
//!
//! **Scrollbars.** A 12 pt horizontal bar along the bottom and a vertical
//! one along the right, meeting in a `chrome` corner square. Each track is
//! `chrome` with a 1 pt `separator` on its inner edge. The thumb is inset
//! 3 pt, fully round, at least 24 pt long, `text_faint` at rest and
//! `text_dim` while hot (dragged, or the pointer within 3 pt of it).
//! Clicking the track pages one screenful toward the click; dragging the
//! thumb scrolls in proportion, with the extent frozen for the drag. The
//! pointer stays the default arrow over the bars.
//!
//! Panel, menu and list scroll areas are egui's own ("thin", Classic
//! "solid"), set in the chrome style ([`crate::chrome::style`]).

use crate::chrome::Chrome;
use egui::{Event, Id, PointerButton, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

/// Bar thickness, thumb inset, least thumb length and hot distance (§3.19).

/// The dot grid's pitch and dot size. Undetermined by the specification
/// (§5.4): one point every 16, which the evidence's sparse dots fit.

/// Paint the canvas surround over `rect`.
pub fn surround(ui: &Ui, rect: Rect) {
    let chrome = Chrome::of(ui.ctx());
    let p = &chrome.palette;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, p.canvas);
    if !chrome.style.canvas_dots || p.canvas_dot == p.canvas {
        return;
    }
    let first = (rect.min.to_vec2() / crate::chrome::Chrome::of(ui.ctx()).style.canvas_dot_pitch as f32).ceil() * crate::chrome::Chrome::of(ui.ctx()).style.canvas_dot_pitch as f32;
    let mut y = first.y;
    while y < rect.bottom() {
        let mut x = first.x;
        while x < rect.right() {
            painter.rect_filled(
                Rect::from_min_size(pos2(x, y), Vec2::splat(crate::chrome::Chrome::of(ui.ctx()).style.canvas_dot_size as f32)),
                0.0,
                p.canvas_dot,
            );
            x += crate::chrome::Chrome::of(ui.ctx()).style.canvas_dot_pitch as f32;
        }
        y += crate::chrome::Chrome::of(ui.ctx()).style.canvas_dot_pitch as f32;
    }
}

/// One scrollbar's geometry along its axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thumb {
    /// Offset of the thumb's start from the track's start, and its length.
    pub start: f32,
    pub length: f32,
}

/// The thumb on a track `track` long, for a view `view` long over content
/// `content` long, scrolled `offset` in.
pub fn thumb(track: f32, view: f32, content: f32, offset: f32) -> Thumb {
    thumb_with_min(track, view, content, offset, crate::style::default_tokens().canvas_thumb_min as f32)
}

fn thumb_with_min(track: f32, view: f32, content: f32, offset: f32, minimum: f32) -> Thumb {
    let content = content.max(view).max(1.0);
    let length = (track * view / content).clamp(minimum.min(track), track);
    let travel = content - view;
    let start = if travel <= 0.0 {
        0.0
    } else {
        (offset / travel).clamp(0.0, 1.0) * (track - length)
    };
    Thumb { start, length }
}

/// One press on a scrollbar, from press to release: whether it landed on
/// the thumb, where along the bar, and the extent frozen for its drag.
#[derive(Clone, Copy, Debug, Default)]
struct Gesture {
    on_thumb: bool,
    origin: f32,
    from: f32,
    view: f32,
    content: f32,
}

/// Scrollbars along the bottom and right of `viewport` for `content` (the
/// document's size in points) scrolled by `offset`, which they change.
/// Returns the part of `viewport` left for the document.
pub fn scrollbars(ui: &mut Ui, id: Id, viewport: Rect, content: Vec2, offset: &mut Vec2) -> Rect {
    let p = Chrome::of(ui.ctx()).palette;
    let inner = Rect::from_min_max(viewport.min, viewport.max - Vec2::splat(crate::chrome::Chrome::of(ui.ctx()).style.canvas_bar as f32));
    let corner = Rect::from_min_max(inner.max, viewport.max);
    ui.painter().rect_filled(corner, 0.0, p.chrome);
    for axis in [0, 1] {
        let track = if axis == 0 {
            Rect::from_min_max(
                pos2(inner.left(), inner.bottom()),
                pos2(inner.right(), viewport.bottom()),
            )
        } else {
            Rect::from_min_max(
                pos2(inner.right(), inner.top()),
                pos2(viewport.right(), inner.bottom()),
            )
        };
        let view = inner.size()[axis];
        bar(
            ui,
            id.with(axis),
            axis,
            track,
            view,
            content[axis],
            &mut offset[axis],
        );
    }
    *offset = offset
        .max(Vec2::ZERO)
        .min((content - inner.size()).max(Vec2::ZERO));
    inner
}

fn bar(ui: &mut Ui, id: Id, axis: usize, track: Rect, view: f32, content: f32, offset: &mut f32) {
    let p = Chrome::of(ui.ctx()).palette;
    let painter = ui.painter();
    painter.rect_filled(track, 0.0, p.chrome);
    let rule = Stroke::new(1.0, p.separator);
    if axis == 0 {
        painter.hline(
            track.x_range(),
            painter.round_to_pixel_center(track.top() + 0.5),
            rule,
        );
    } else {
        painter.vline(
            painter.round_to_pixel_center(track.left() + 0.5),
            track.y_range(),
            rule,
        );
    }
    let lane = track.shrink(crate::chrome::Chrome::of(ui.ctx()).style.canvas_thumb_inset as f32);
    let along = |r: Rect| {
        if axis == 0 {
            (r.left(), r.width())
        } else {
            (r.top(), r.height())
        }
    };
    let (lane_start, lane_len) = along(lane);

    let response = ui.interact(track, id, Sense::click_and_drag());
    let mut gesture: Option<Gesture> = ui.data(|d| d.get_temp(id));
    let t = thumb_with_min(lane_len, view, content, *offset, Chrome::of(ui.ctx()).style.canvas_thumb_min as f32);
    let thumb_rect = |t: Thumb| {
        if axis == 0 {
            Rect::from_min_size(
                pos2(lane_start + t.start, lane.top()),
                vec2(t.length, lane.height()),
            )
        } else {
            Rect::from_min_size(
                pos2(lane.left(), lane_start + t.start),
                vec2(lane.width(), t.length),
            )
        }
    };
    let rect = thumb_rect(t);
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let coord = |pos: Pos2| if axis == 0 { pos.x } else { pos.y };

    // The gesture is decided where the press lands, from the press event
    // itself (a press and its release can share a frame), and kept until
    // the button comes up: a press on the thumb drags it however far the
    // pointer has gone by the time egui calls it a drag, and only a press
    // on the bare track pages.
    let press = ui.input(|i| {
        i.events.iter().find_map(|event| match event {
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                ..
            } if track.contains(*pos) => Some(*pos),
            _ => None,
        })
    });
    if let Some(pos) = press {
        gesture = Some(Gesture {
            on_thumb: rect.expand(crate::chrome::Chrome::of(ui.ctx()).style.hot_distance as f32).contains(pos),
            origin: coord(pos),
            from: *offset,
            view,
            content,
        });
    }
    if let Some(g) = gesture {
        if g.on_thumb
            && (response.dragged() || response.is_pointer_button_down_on())
            && let Some(pos) = pointer
        {
            let frozen = thumb_with_min(lane_len, g.view, g.content, g.from, Chrome::of(ui.ctx()).style.canvas_thumb_min as f32);
            let room = (lane_len - frozen.length).max(1.0);
            *offset = g.from + (coord(pos) - g.origin) / room * (g.content - g.view).max(0.0);
        }
        if response.clicked() && !g.on_thumb {
            // A page toward the press.
            let before = g.origin < along(rect).0;
            *offset += if before { -view } else { view };
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            gesture = None;
        }
    }
    *offset = offset.clamp(0.0, (content - view).max(0.0));
    let dragging = gesture.is_some_and(|g| g.on_thumb);
    let hot = dragging || pointer.is_some_and(|pos| rect.expand(crate::chrome::Chrome::of(ui.ctx()).style.hot_distance as f32).contains(pos));
    let shown = thumb_rect(thumb_with_min(lane_len, view, content, *offset, Chrome::of(ui.ctx()).style.canvas_thumb_min as f32));
    let radius = shown.width().min(shown.height()) / 2.0;
    ui.painter()
        .rect_filled(shown, radius, if hot { p.text_dim } else { p.text_faint });
    ui.data_mut(|d| match gesture {
        Some(gesture) => {
            d.insert_temp(id, gesture);
        }
        None => d.remove::<Gesture>(id),
    });
}


/// The installed value of the former `BAR` constant.
pub fn bar_of(ctx: &egui::Context) -> f32 { crate::chrome::Chrome::of(ctx).style.canvas_bar as f32 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thumb_is_proportional_and_never_shorter_than_24() {
        assert_eq!(
            thumb(200.0, 100.0, 400.0, 0.0),
            Thumb {
                start: 0.0,
                length: 50.0
            }
        );
        assert_eq!(
            thumb(200.0, 100.0, 400.0, 300.0),
            Thumb {
                start: 150.0,
                length: 50.0
            }
        );
        assert_eq!(thumb(200.0, 10.0, 10_000.0, 0.0).length, 24.0);
        assert_eq!(
            thumb(200.0, 500.0, 400.0, 0.0),
            Thumb {
                start: 0.0,
                length: 200.0
            },
            "content smaller than the view"
        );
    }
}
