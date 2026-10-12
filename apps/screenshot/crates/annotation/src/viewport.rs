// SPDX-License-Identifier: MIT OR Apache-2.0
//! One image-pixel transform for painting and pointer mapping. Zoom and pan
//! are view state; the document only ever sees image coordinates.
use crate::document::Point;

/// The shown image inside a view of `view` size: fitted, then zoomed about
/// the centre and panned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// View pixels per image pixel.
    pub scale: f32,
    /// Where the image's top-left lands in the view.
    pub origin: Point,
    pub width: f32,
    pub height: f32,
}

/// The zoom range, relative to fit.
pub const ZOOM: std::ops::RangeInclusive<f32> = 0.1..=8.0;

impl Viewport {
    pub fn fit(image: (u32, u32), view: (f32, f32), zoom: f32, pan: Point) -> Self {
        let scale = (view.0 / image.0.max(1) as f32)
            .min(view.1 / image.1.max(1) as f32)
            .max(0.0001)
            * zoom.clamp(*ZOOM.start(), *ZOOM.end());
        let width = image.0 as f32 * scale;
        let height = image.1 as f32 * scale;
        Self {
            scale,
            origin: Point {
                x: (view.0 - width) / 2.0 + pan.x,
                y: (view.1 - height) / 2.0 + pan.y,
            },
            width,
            height,
        }
    }

    /// The image point under view point `p`.
    pub fn image(self, p: Point) -> Point {
        Point {
            x: (p.x - self.origin.x) / self.scale,
            y: (p.y - self.origin.y) / self.scale,
        }
    }

    /// The view point over image point `p`.
    pub fn view(self, p: Point) -> Point {
        Point {
            x: p.x * self.scale + self.origin.x,
            y: p.y * self.scale + self.origin.y,
        }
    }

    /// Whether view point `p` is over the image.
    pub fn contains(self, p: Point) -> bool {
        let p = self.image(p);
        p.x >= 0.0
            && p.y >= 0.0
            && p.x <= self.width / self.scale
            && p.y <= self.height / self.scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_zoom_and_pan_are_invertible() {
        let v = Viewport::fit(
            (3840, 2160),
            (911.0, 517.0),
            1.7,
            Point { x: 11.5, y: -3.0 },
        );
        let p = Point { x: 101.5, y: 777.0 };
        let q = v.image(v.view(p));
        assert!((q.x - p.x).abs() < 0.001 && (q.y - p.y).abs() < 0.001);
        assert!(!v.contains(Point {
            x: -10000.0,
            y: 0.0
        }));
    }

    #[test]
    fn fit_centres_and_zoom_is_clamped() {
        let v = Viewport::fit((200, 100), (400.0, 400.0), 1.0, Point { x: 0.0, y: 0.0 });
        assert_eq!(v.scale, 2.0);
        assert_eq!(v.origin, Point { x: 0.0, y: 100.0 });
        let far = Viewport::fit((200, 100), (400.0, 400.0), 100.0, Point { x: 0.0, y: 0.0 });
        assert_eq!(far.scale, 16.0);
    }
}
