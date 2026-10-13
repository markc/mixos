// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Scale arithmetic for layer surfaces.

/// `wp_fractional_scale_v1` sends the scale in 120ths.
pub(crate) fn from_120ths(value: u32) -> f32 {
    (value as f32 / 120.0).max(f32::EPSILON)
}

/// The buffer size for a logical size at `scale`: rounded, never 0 on a
/// non-empty side.
pub(crate) fn buffer_size(logical: (u32, u32), scale: f32) -> (u32, u32) {
    let side = |n: u32| {
        if n == 0 {
            0
        } else {
            ((n as f32 * scale).round() as u32).max(1)
        }
    };
    (side(logical.0), side(logical.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_scales_and_buffer_sizes() {
        assert_eq!(from_120ths(120), 1.0);
        assert_eq!(from_120ths(300), 2.5);
        assert_eq!(from_120ths(180), 1.5);
        assert!(from_120ths(0) > 0.0);
        assert_eq!(buffer_size((1536, 864), 2.5), (3840, 2160));
        assert_eq!(buffer_size((1280, 800), 1.0), (1280, 800));
        assert_eq!(buffer_size((3, 3), 1.5), (5, 5));
        assert_eq!(buffer_size((0, 10), 2.0), (0, 20));
        assert_eq!(buffer_size((1, 1), 0.1), (1, 1));
    }
}
