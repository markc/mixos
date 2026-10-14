// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! How an image fills a surface: centre-crop, the default and only fit for
//! now. The image keeps its aspect ratio and covers the whole surface; the
//! overflow on its long axis is cut equally from both ends. Fit modes,
//! slideshows and folders come later.

/// A part of an image in texture coordinates: 0 at the top-left, 1 at the
/// bottom-right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Uv {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

/// The whole image.
pub const FULL: Uv = Uv {
    min: [0.0, 0.0],
    max: [1.0, 1.0],
};

/// The part of an image of `image` pixels that centre-crop shows on a
/// surface of `surface` logical pixels.
pub fn centre_crop(image: [u32; 2], surface: [f32; 2]) -> Uv {
    let [iw, ih] = [image[0] as f32, image[1] as f32];
    let [sw, sh] = surface;
    if iw <= 0.0 || ih <= 0.0 || sw <= 0.0 || sh <= 0.0 {
        return FULL;
    }
    let (image_aspect, surface_aspect) = (iw / ih, sw / sh);
    if image_aspect > surface_aspect {
        // Wider than the surface: cut the sides.
        let inset = (1.0 - surface_aspect / image_aspect) / 2.0;
        Uv {
            min: [inset, 0.0],
            max: [1.0 - inset, 1.0],
        }
    } else {
        // Taller than the surface (or the same): cut the top and bottom.
        let inset = (1.0 - image_aspect / surface_aspect) / 2.0;
        Uv {
            min: [0.0, inset],
            max: [1.0, 1.0 - inset],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_crop_cuts_the_overflow_equally_from_both_ends() {
        // A 2:1 image on a square: the middle half of its width shows.
        let uv = centre_crop([200, 100], [100.0, 100.0]);
        assert_eq!(uv.min, [0.25, 0.0]);
        assert_eq!(uv.max, [0.75, 1.0]);
        // A 1:2 image on a 2:1 surface: the middle half of its height shows.
        let uv = centre_crop([100, 200], [200.0, 100.0]);
        assert_eq!(uv.min, [0.0, 0.375]);
        assert_eq!(uv.max, [1.0, 0.625]);
    }

    #[test]
    fn a_matching_aspect_or_a_degenerate_size_shows_the_whole_image() {
        assert_eq!(centre_crop([16, 9], [160.0, 90.0]), FULL);
        assert_eq!(centre_crop([0, 9], [1.0, 1.0]), FULL);
        assert_eq!(centre_crop([16, 9], [0.0, 90.0]), FULL);
    }
}
