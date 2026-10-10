// SPDX-License-Identifier: MIT OR Apache-2.0
//! PRIVATE STUB. The accent is not yet a settings field, so it is taken from
//! the resolved design: the accent pair's rendered surface, quantised to 8-bit
//! sRGB exactly as the toolkit's selection fill shows it. Remove this module
//! when the design publishes a first-class accent. Nothing outside settingsd
//! may depend on this function's shape.
use design::LinearRgba;
use settings::Effective;

/// Accent as sRGB 0.0..=1.0 per channel, or `None` when the resolved design
/// carries no accent pair.
pub(crate) fn accent_srgb(effective: &Effective) -> Option<[f64; 3]> {
    let [red, green, blue, alpha] = effective.design.pairs.get("accent")?.rendered_surface;
    let [r, g, b, _] = LinearRgba {
        red,
        green,
        blue,
        alpha,
    }
    .to_srgba8();
    Some([r, g, b].map(|channel| f64::from(channel) / 255.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use settings::{Desktop, resolve};

    #[test]
    fn default_desktop_accent_is_eight_bit_srgb_within_range() {
        let effective = resolve(&Desktop::default()).unwrap();
        let accent = accent_srgb(&effective["desktop"]).expect("accent pair present");
        for channel in accent {
            assert!((0.0..=1.0).contains(&channel));
            let eight = channel * 255.0;
            assert!((eight - eight.round()).abs() < 1e-9, "quantised to 8 bits");
        }
    }

    #[test]
    fn missing_accent_pair_is_none() {
        let mut effective = resolve(&Desktop::default()).unwrap()["desktop"].clone();
        effective.design.pairs.remove("accent");
        assert!(accent_srgb(&effective).is_none());
    }
}
