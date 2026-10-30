// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared top-level window geometry and pixels. Extracted from compd's decor
//! presets/raster; no window-system or GPU types belong here.
use crate::{ResolvedDictionary, ResolvedMetricKind, WindowStyle};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowSpec {
    pub softness: f32,
    pub offset_y: f32,
    pub color: [f32; 4],
    pub alpha_focused: f32,
    pub alpha_unfocused: f32,
}

impl ShadowSpec {
    pub fn for_style(style: WindowStyle) -> Self {
        let (softness, offset_y, alpha_focused, alpha_unfocused) = match style {
            WindowStyle::Mac => (40.0, 12.0, 0.45, 0.22),
            WindowStyle::Win11 => (22.0, 6.0, 0.30, 0.15),
            WindowStyle::Mixos => (28.0, 8.0, 0.35, 0.18),
        };
        Self {
            softness,
            offset_y,
            color: [0.0, 0.0, 0.0, 1.0],
            alpha_focused,
            alpha_unfocused,
        }
    }

    pub fn alpha(self, focused: bool) -> f32 {
        if focused {
            self.alpha_focused
        } else {
            self.alpha_unfocused
        }
    }
}

pub fn corner_radius(style: WindowStyle, dictionary: Option<&ResolvedDictionary>) -> f32 {
    let preset = match style {
        WindowStyle::Mac => 12.0,
        WindowStyle::Win11 => 8.0,
        WindowStyle::Mixos => 10.0,
    };
    resolve_corner_radius(style, preset, dictionary)
}

/// Preserve a customised preset when there is no applicable metric, as decor
/// did before the extraction. Platform styles always retain their own radius.
pub fn resolve_corner_radius(
    style: WindowStyle,
    preset: f32,
    dictionary: Option<&ResolvedDictionary>,
) -> f32 {
    if style == WindowStyle::Mixos {
        dictionary
            .and_then(|d| d.metrics.get("radius"))
            .filter(|r| r.kind == ResolvedMetricKind::Px)
            .map_or(preset, |r| r.value as f32)
    } else {
        preset
    }
}

pub fn border_width(style: WindowStyle) -> f32 {
    if style == WindowStyle::Mac { 0.0 } else { 1.0 }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub radius: f32,
    pub border_width: f32,
    /// Straight sRGB, just as compd resolves its non-text border/ring slots.
    pub border_focused: [f32; 4],
    pub border_unfocused: [f32; 4],
    pub shadow: ShadowSpec,
}

impl Frame {
    pub fn resolve(style: WindowStyle, dictionary: &ResolvedDictionary) -> Self {
        let colour = |name| {
            dictionary.colours.non_text.get(name).map_or([0.0; 4], |c| {
                c.value.to_srgba8().map(|v| f32::from(v) / 255.0)
            })
        };
        Self {
            radius: corner_radius(style, Some(dictionary)),
            border_width: border_width(style),
            border_focused: colour(if style == WindowStyle::Mixos {
                "ring"
            } else {
                "border"
            }),
            border_unfocused: colour("border"),
            shadow: ShadowSpec::for_style(style),
        }
    }
}

impl Default for Frame {
    fn default() -> Self {
        let style = WindowStyle::default();
        Self {
            radius: corner_radius(style, None),
            border_width: border_width(style),
            border_focused: [0.0; 4],
            border_unfocused: [0.0; 4],
            shadow: ShadowSpec::for_style(style),
        }
    }
}

/// Signed distance in pixels, negative inside. Kept bit-for-bit from decor.
pub fn rounded_box(
    p: (f32, f32),
    origin: (f32, f32),
    size: (f32, f32),
    top: f32,
    bottom: f32,
) -> f32 {
    let (hw, hh) = (size.0 * 0.5, size.1 * 0.5);
    let (qx, qy) = (p.0 - origin.0 - hw, p.1 - origin.1 - hh);
    let r = if qy < 0.0 { top } else { bottom }.min(hw).min(hh).max(0.0);
    let (dx, dy) = (qx.abs() - hw + r, qy.abs() - hh + r);
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    outside + dx.max(dy).min(0.0) - r
}

pub fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

pub struct ShadowTile {
    pub size: u32,
    pub corner: u32,
    pub pixels: Vec<[f32; 4]>,
}

/// Compd's original rounded-box smoothstep raster, including its exact float
/// operation order. The centre is dense; the nine slices stretch independently.
pub fn shadow_tile(colour: [f32; 4], alpha: f32, radius: f32, softness: f32) -> ShadowTile {
    let corner = (softness + radius).ceil().max(1.0) as u32;
    let size = corner * 2 + 1;
    let mut pixels = vec![[0.0; 4]; size as usize * size as usize];
    let inner = (softness, softness);
    let box_size = (size as f32 - 2.0 * softness, size as f32 - 2.0 * softness);
    for y in 0..size {
        for x in 0..size {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let d = rounded_box(p, inner, box_size, radius, radius);
            let t = if softness <= 0.0 {
                coverage(d)
            } else {
                let s = (d / softness).clamp(0.0, 1.0);
                1.0 - s * s * (3.0 - 2.0 * s)
            };
            let a = colour[3] * (alpha * t);
            pixels[(y * size + x) as usize] = [colour[0] * a, colour[1] * a, colour[2] * a, a];
        }
    }
    ShadowTile {
        size,
        corner,
        pixels,
    }
}

pub struct ShadowPiece {
    pub width: i32,
    pub height: i32,
    pub argb: Vec<u8>,
}

pub struct ShadowPieces {
    /// Clockwise from top, as in the KWin protocol's eight buffer attachments.
    pub pieces: Vec<ShadowPiece>,
    pub extents: [i32; 4],
}

/// Eight slices from the same shadow compd paints. Corners include their
/// inward radius and the vertical offset. Cut out the *unshifted* client
/// rounded rect, retaining shadow beneath transparent corner pixels.
pub fn shadow_pieces(
    spec: ShadowSpec,
    radius: f32,
    scale: f32,
    focused: bool,
    size: [u32; 2],
) -> ShadowPieces {
    let scale = scale.clamp(0.5, 8.0);
    let radius = (radius * scale)
        .min(size[0] as f32 / 2.0)
        .min(size[1] as f32 / 2.0)
        .max(0.0);
    let tile = shadow_tile(
        spec.color,
        spec.alpha(focused),
        radius,
        spec.softness * scale,
    );
    let c = tile.corner as i32;
    let soft = (spec.softness * scale).round() as i32;
    let offset = (spec.offset_y * scale).round() as i32;
    let [w, h] = size.map(|v| v as i32);
    // source x/y, width/height, destination x/y. Edge samples are far from
    // the client's corners; its one-pixel centre strip stretches in KWin.
    let slices = [
        (c, 0, 1, c, w / 2, -soft + offset),
        (c + 1, 0, c, c, w + soft - c, -soft + offset),
        (c + 1, c, c, 1, w + soft - c, h / 2),
        (c + 1, c + 1, c, c, w + soft - c, h + soft + offset - c),
        (c, c + 1, 1, c, w / 2, h + soft + offset - c),
        (0, c + 1, c, c, -soft, h + soft + offset - c),
        (0, c, c, 1, -soft, h / 2),
        (0, 0, c, c, -soft, -soft + offset),
    ];
    let pieces = slices
        .into_iter()
        .map(|(sx, sy, width, height, dx, dy)| {
            let mut argb = Vec::with_capacity((width * height * 4) as usize);
            for y in 0..height {
                for x in 0..width {
                    let pixel =
                        tile.pixels[((sy + y) as u32 * tile.size + (sx + x) as u32) as usize];
                    let cut = 1.0
                        - coverage(rounded_box(
                            (dx as f32 + x as f32 + 0.5, dy as f32 + y as f32 + 0.5),
                            (0.0, 0.0),
                            (w as f32, h as f32),
                            radius,
                            radius,
                        ));
                    let q = |v: f32| ((v * cut).clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
                    let value =
                        q(pixel[3]) << 24 | q(pixel[0]) << 16 | q(pixel[1]) << 8 | q(pixel[2]);
                    argb.extend_from_slice(&value.to_ne_bytes());
                }
            }
            ShadowPiece {
                width,
                height,
                argb,
            }
        })
        .collect();
    ShadowPieces {
        pieces,
        extents: [soft, soft - offset, soft, soft + offset],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pieces_retain_shadow_in_transparent_corners_and_cut_out_the_client() {
        for style in WindowStyle::ALL {
            let spec = ShadowSpec::for_style(style);
            for scale in [1.0, 1.5, 2.0] {
                let radius = corner_radius(style, None);
                let tiles = shadow_pieces(spec, radius, scale, true, [640, 480]);
                let dim = ((spec.softness + radius) * scale).ceil() as i32;
                assert_eq!((tiles.pieces[7].width, tiles.pieces[7].height), (dim, dim));
                let alpha = |x: i32, y: i32| {
                    let p = &tiles.pieces[7];
                    u32::from_ne_bytes(
                        p.argb[((y * p.width + x) * 4) as usize..][..4]
                            .try_into()
                            .unwrap(),
                    ) >> 24
                };
                let left = tiles.extents[0];
                let top = tiles.extents[1];
                assert!(
                    alpha(left, top) > 0,
                    "shadow fills the transparent window corner"
                );
                assert_eq!(
                    alpha(dim - 1, dim - 1),
                    0,
                    "opaque client interior is cut out"
                );
                assert!(tiles.pieces.iter().any(|p| p.argb.iter().any(|b| *b != 0)));
                let unfocused = shadow_pieces(spec, radius, scale, false, [640, 480]);
                assert!(tiles.pieces.iter().zip(&unfocused.pieces).all(|(a, b)| {
                    a.argb
                        .chunks_exact(4)
                        .zip(b.argb.chunks_exact(4))
                        .all(|(a, b)| {
                            u32::from_ne_bytes(a.try_into().unwrap()) >> 24
                                >= u32::from_ne_bytes(b.try_into().unwrap()) >> 24
                        })
                }));
            }
        }
    }
}
