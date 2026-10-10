// SPDX-License-Identifier: MIT OR Apache-2.0
//! Editor geometry: gutter, cell grid, row and column arithmetic,
//! scrollbars. Pure, so it is tested with plain metrics.
//!
//! The text grid starts at `bounds.left() + gutter_w` and `bounds.top()`:
//! visual row `i` (counted from the top of the view) is drawn at
//! `top + i · row_h`, and cell `c` at `left + gutter_w + (c − origin) · cell_w`,
//! where `origin` is the horizontal scroll without wrap and the row's first
//! cell with it.

use egui::{Pos2, Rect, pos2, vec2};

/// Width of the origin strip in the gutter: lines other origins changed.
pub const STRIP_W: f32 = 4.0;
/// Overlay scrollbar thickness.
pub const SCROLLBAR_W: f32 = 10.0;
/// Smallest scrollbar thumb.
pub const MIN_THUMB: f32 = 24.0;
/// Rows kept between the caret and the top or bottom edge when following it.
pub const CARET_MARGIN_ROWS: usize = 2;
/// Cells kept between the caret and the left or right edge when following it.
pub const CARET_MARGIN_CELLS: usize = 4;

/// Font-derived sizes, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub cell_w: f32,
    pub row_h: f32,
}

/// Where everything sits for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// The whole widget.
    pub bounds: Rect,
    pub metrics: Metrics,
    /// Digits reserved for line numbers (0 when they are hidden).
    pub digits: usize,
    pub gutter_w: f32,
}

impl Geometry {
    pub fn new(bounds: Rect, metrics: Metrics, line_count: usize, line_numbers: bool) -> Self {
        let digits = if line_numbers {
            digits(line_count).max(3)
        } else {
            0
        };
        Self {
            bounds,
            metrics,
            digits,
            gutter_w: gutter_width(metrics, digits),
        }
    }

    /// The text area, right of the gutter.
    pub fn text_rect(&self) -> Rect {
        Rect::from_min_max(
            pos2(
                (self.bounds.left() + self.gutter_w).min(self.bounds.right()),
                self.bounds.top(),
            ),
            self.bounds.max,
        )
    }

    pub fn gutter_rect(&self) -> Rect {
        Rect::from_min_size(
            self.bounds.min,
            vec2(self.gutter_w.min(self.bounds.width()), self.bounds.height()),
        )
    }

    /// Rows that fit entirely (Page Up and Page Down move by this).
    pub fn full_rows(&self) -> usize {
        ((self.bounds.height() / self.metrics.row_h).floor() as usize).max(1)
    }

    /// Rows drawn, including a partial last one.
    pub fn drawn_rows(&self) -> usize {
        (self.bounds.height() / self.metrics.row_h).ceil() as usize
    }

    /// Whole cells that fit in the text area, less the scrollbar.
    pub fn cols(&self) -> usize {
        (((self.text_rect().width() - SCROLLBAR_W) / self.metrics.cell_w).floor() as usize).max(1)
    }

    /// The top of visual row `i`.
    pub fn row_y(&self, i: usize) -> f32 {
        self.bounds.top() + i as f32 * self.metrics.row_h
    }

    /// The left of cell `cells` on a row whose first drawn cell is `origin`.
    pub fn cell_x(&self, cells: usize, origin: usize) -> f32 {
        self.bounds.left() + self.gutter_w + (cells as f32 - origin as f32) * self.metrics.cell_w
    }

    /// The visual row under `p` (may be past the drawn rows; 0 above the
    /// view) and the fractional cell from the row's origin.
    pub fn hit(&self, p: Pos2) -> (isize, f32) {
        let row = ((p.y - self.bounds.top()) / self.metrics.row_h).floor() as isize;
        let cells = ((p.x - self.bounds.left() - self.gutter_w) / self.metrics.cell_w).max(0.0);
        (row, cells)
    }

    /// Vertical scrollbar track (overlay, right edge).
    pub fn vbar_track(&self) -> Rect {
        let t = self.text_rect();
        Rect::from_min_max(pos2(t.right() - SCROLLBAR_W, t.top()), t.max)
    }

    /// Horizontal scrollbar track (overlay, bottom edge, left of the vbar).
    pub fn hbar_track(&self) -> Rect {
        let t = self.text_rect();
        Rect::from_min_max(
            pos2(t.left(), t.bottom() - SCROLLBAR_W),
            pos2((t.right() - SCROLLBAR_W).max(t.left()), t.bottom()),
        )
    }
}

/// Gutter: right-aligned line numbers plus a cell of padding, the origin
/// strip, a lint column one cell wide, and half a cell before the text.
pub fn gutter_width(m: Metrics, digits: usize) -> f32 {
    let numbers = if digits > 0 {
        (digits as f32 + 1.0) * m.cell_w
    } else {
        0.0
    };
    (numbers + STRIP_W + m.cell_w + m.cell_w * 0.5).round()
}

pub fn digits(n: usize) -> usize {
    n.max(1).ilog10() as usize + 1
}

/// A scrollbar thumb along a track: `(offset, length)` for a view of
/// `visible` units at `first` of `total`. `None` when everything fits.
pub fn thumb(track_len: f32, total: usize, visible: usize, first: usize) -> Option<(f32, f32)> {
    if total <= visible || track_len <= 0.0 {
        return None;
    }
    let len =
        (track_len * visible as f32 / total as f32).clamp(MIN_THUMB.min(track_len), track_len);
    let range = (total - visible) as f32;
    let off = (track_len - len) * (first.min(total - visible) as f32 / range);
    Some((off, len))
}

/// The first unit shown when the thumb's leading edge is at `offset`.
pub fn thumb_to_first(track_len: f32, total: usize, visible: usize, offset: f32) -> usize {
    let Some((_, len)) = thumb(track_len, total, visible, 0) else {
        return 0;
    };
    let free = (track_len - len).max(1.0);
    ((offset / free).clamp(0.0, 1.0) * (total - visible) as f32).round() as usize
}

/// The horizontal scroll that keeps cell `cells` in a view of `cols`.
pub fn follow_x(x_cells: usize, cells: usize, cols: usize) -> usize {
    let margin = CARET_MARGIN_CELLS.min(cols.saturating_sub(1) / 2);
    if cells < x_cells + margin {
        cells.saturating_sub(margin)
    } else if cells + margin >= x_cells + cols {
        (cells + margin + 1).saturating_sub(cols)
    } else {
        x_cells
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: Metrics = Metrics {
        cell_w: 10.0,
        row_h: 20.0,
    };

    fn geo() -> Geometry {
        Geometry::new(
            Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 405.0)),
            M,
            1234,
            true,
        )
    }

    #[test]
    fn gutter_and_rows() {
        let g = geo();
        assert_eq!(g.digits, 4);
        assert_eq!(g.gutter_w, (5.0 * 10.0 + STRIP_W + 10.0 + 5.0_f32).round());
        assert_eq!(g.full_rows(), 20);
        assert_eq!(g.drawn_rows(), 21);
        assert_eq!(
            Geometry::new(g.bounds, M, 5, true).digits,
            3,
            "at least three digits"
        );
        assert_eq!(
            Geometry::new(g.bounds, M, 5, false).gutter_w,
            (STRIP_W + 15.0_f32).round()
        );
    }

    #[test]
    fn hit_is_the_inverse_of_positioning() {
        let g = geo();
        for (row, cells) in [(0, 2), (2, 5), (13, 40)] {
            let p = pos2(g.cell_x(cells, 0) + 1.0, g.row_y(row) + 1.0);
            let (r, c) = g.hit(p);
            assert_eq!(r, row as isize);
            assert_eq!(c.floor() as usize, cells);
        }
        assert_eq!(g.cell_x(4, 3), 100.0 + g.gutter_w + 10.0);
        let (r, c) = g.hit(pos2(0.0, -500.0));
        assert!(r < 0 && c == 0.0, "above and left of the view");
    }

    #[test]
    fn horizontal_follow_keeps_margins() {
        assert_eq!(follow_x(0, 10, 80), 0, "inside: unchanged");
        assert_eq!(follow_x(0, 100, 80), 25);
        assert_eq!(follow_x(50, 10, 80), 6);
    }

    #[test]
    fn thumbs() {
        assert_eq!(thumb(100.0, 10, 20, 0), None);
        assert_eq!(
            thumb(100.0, 100, 20, 0),
            Some((0.0, 24.0)),
            "20%, raised to the minimum"
        );
        assert_eq!(thumb(100.0, 100, 20, 80).map(|t| t.0), Some(76.0));
        assert_eq!(thumb_to_first(100.0, 100, 20, 76.0), 80);
        assert_eq!(thumb_to_first(100.0, 100, 20, 38.0), 40);
    }
}
