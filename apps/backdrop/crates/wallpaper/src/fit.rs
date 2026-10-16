// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Pure image placement in logical coordinates. Native sizing uses the source
//! dimensions, even when the retained texture has been downscaled.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    #[default]
    Fill,
    Fit,
    Centre,
    Tile,
    Stretch,
}

/// A destination rectangle relative to the surface's top left, with the
/// corresponding texture coordinates. Tile uses UVs beyond one with a repeat
/// sampler, so its geometry has constant cost regardless of the tile count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub uv: Uv,
}

pub fn placement(
    image: [u32; 2],
    surface: [f32; 2],
    pixels_per_point: f32,
    mode: Fit,
) -> Placement {
    let empty = Placement {
        min: [0.0; 2],
        max: [0.0; 2],
        uv: FULL,
    };
    if image.contains(&0)
        || surface.iter().any(|v| !v.is_finite() || *v <= 0.0)
        || !pixels_per_point.is_finite()
        || pixels_per_point <= 0.0
    {
        return empty;
    }
    // An extreme but finite scale can still overflow or underflow the maths
    // below (`f32::MAX` points per pixel): never return a non-finite rectangle.
    let placed = placement_of(image, surface, pixels_per_point, mode);
    let finite = placed
        .min
        .iter()
        .chain(&placed.max)
        .chain(&placed.uv.min)
        .chain(&placed.uv.max)
        .all(|v| v.is_finite());
    if finite { placed } else { empty }
}

fn placement_of(
    image: [u32; 2],
    surface: [f32; 2],
    pixels_per_point: f32,
    mode: Fit,
) -> Placement {
    let whole = Placement {
        min: [0.0; 2],
        max: surface,
        uv: FULL,
    };
    let native = [
        image[0] as f32 / pixels_per_point,
        image[1] as f32 / pixels_per_point,
    ];
    match mode {
        Fit::Fill => Placement {
            uv: centre_crop(image, surface),
            ..whole
        },
        Fit::Stretch => whole,
        Fit::Tile => Placement {
            uv: Uv {
                min: [0.0; 2],
                max: [surface[0] / native[0], surface[1] / native[1]],
            },
            ..whole
        },
        Fit::Fit | Fit::Centre => {
            let factor = if mode == Fit::Fit {
                (surface[0] / native[0]).min(surface[1] / native[1])
            } else {
                1.0
            };
            let size = [native[0] * factor, native[1] * factor];
            let origin = [(surface[0] - size[0]) / 2.0, (surface[1] - size[1]) / 2.0];
            let min = [origin[0].max(0.0), origin[1].max(0.0)];
            let max = [
                (origin[0] + size[0]).min(surface[0]),
                (origin[1] + size[1]).min(surface[1]),
            ];
            Placement {
                min,
                max,
                uv: Uv {
                    min: [
                        (min[0] - origin[0]) / size[0],
                        (min[1] - origin[1]) / size[1],
                    ],
                    max: [
                        (max[0] - origin[0]) / size[0],
                        (max[1] - origin[1]) / size[1],
                    ],
                },
            }
        }
    }
}

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
    if iw <= 0.0 || ih <= 0.0 || sw <= 0.0 || sh <= 0.0 || !sw.is_finite() || !sh.is_finite() {
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

    #[test]
    fn fit_letterboxes_wide_and_tall_images() {
        let wide = placement([200, 100], [100.0, 100.0], 1.0, Fit::Fit);
        assert_eq!(
            (wide.min, wide.max, wide.uv),
            ([0.0, 25.0], [100.0, 75.0], FULL)
        );
        let tall = placement([100, 200], [100.0, 100.0], 2.5, Fit::Fit);
        assert_eq!(
            (tall.min, tall.max, tall.uv),
            ([25.0, 0.0], [75.0, 100.0], FULL)
        );
    }

    #[test]
    fn native_centre_honours_fractional_scale_and_clips_oversize_images() {
        let small = placement([50, 25], [100.0, 100.0], 2.5, Fit::Centre);
        assert_eq!((small.min, small.max), ([40.0, 45.0], [60.0, 55.0]));
        let large = placement([400, 200], [100.0, 100.0], 2.0, Fit::Centre);
        assert_eq!((large.min, large.max), ([0.0, 0.0], [100.0, 100.0]));
        assert_eq!(
            large.uv,
            Uv {
                min: [0.25, 0.0],
                max: [0.75, 1.0]
            }
        );
    }

    #[test]
    fn tile_has_one_quad_at_the_native_pixel_period() {
        let tile = placement([32, 16], [100.0, 50.0], 2.0, Fit::Tile);
        assert_eq!((tile.min, tile.max), ([0.0, 0.0], [100.0, 50.0]));
        assert_eq!(tile.uv.max, [6.25, 6.25]);
    }

    #[test]
    fn stretch_and_fill_cover_the_surface_with_different_uvs() {
        let stretch = placement([200, 100], [100.0, 100.0], 1.0, Fit::Stretch);
        let fill = placement([200, 100], [100.0, 100.0], 1.0, Fit::Fill);
        assert_eq!(stretch.uv, FULL);
        assert_eq!((stretch.min, stretch.max), (fill.min, fill.max));
        assert_eq!(fill.uv, centre_crop([200, 100], [100.0, 100.0]));
    }

    #[test]
    fn invalid_sizes_produce_empty_geometry() {
        for (image, surface, scale) in [
            ([0, 10], [10.0, 10.0], 1.0),
            ([10, 10], [f32::NAN, 10.0], 1.0),
            ([10, 10], [10.0, 10.0], 0.0),
            ([10, 10], [10.0, 10.0], f32::INFINITY),
        ] {
            let p = placement(image, surface, scale, Fit::Fit);
            assert_eq!((p.min, p.max), ([0.0; 2], [0.0; 2]));
        }
    }

    #[test]
    fn extreme_finite_scales_never_produce_non_finite_geometry() {
        for mode in [Fit::Fill, Fit::Fit, Fit::Centre, Fit::Tile, Fit::Stretch] {
            for scale in [f32::MAX, f32::MIN_POSITIVE] {
                let p = placement([1, 1], [3840.0, 2160.0], scale, mode);
                let values = p.min.iter().chain(&p.max).chain(&p.uv.min).chain(&p.uv.max);
                for v in values {
                    assert!(v.is_finite(), "{mode:?} at {scale}: {p:?}");
                }
            }
        }
    }
}
