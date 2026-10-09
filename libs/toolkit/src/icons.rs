// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lucide icons: unmodified upstream SVGs (Lucide 1.54.0, ISC; NOTICE),
//! rendered through egui's SVG loader.
//!
//! **Stroke weight is a token.** Lucide draws every icon on a 24 px grid with
//! `stroke-width="2"`. [`stroke_width`] reads the design metric
//! `icon.stroke_width` and falls back to [`DEFAULT_STROKE`]; each icon's
//! stroke is rewritten to it when the image is built, so a lighter icon set
//! (for a light type profile) is a design change, not an asset change.
//!
//! **Colour comes from the theme.** Upstream SVGs stroke in `currentColor`.
//! The loader has no current colour, so the stroke is rendered as a white
//! mask ([`MASK`]) and tinted at draw time with the widget's text colour.
//! The mask is a rendering device, not a UI colour: every visible colour is
//! the tint.

use crate::theme::Theme;
use design::ResolvedMetricKind;
use egui::{Color32, Image, ImageSource, Vec2};
use std::borrow::Cow;

/// Lucide's own stroke width, used when the design authors none.
pub const DEFAULT_STROKE: f32 = 2.0;

/// The stroke colour the SVG is rasterised in before tinting. White, so
/// multiplying by the tint yields the tint exactly.
pub const MASK: &str = "#fff";

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        /// The icons the toolkit ships. Add an icon by copying its SVG from
        /// the pinned Lucide release into `assets/icons/` and listing it here.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon { $($variant),+ }

        impl Icon {
            /// Every shipped icon.
            pub const ALL: &[Icon] = &[$(Icon::$variant),+];

            /// The Lucide name.
            pub fn name(self) -> &'static str {
                match self { $(Icon::$variant => $file),+ }
            }

            fn svg(self) -> &'static str {
                match self {
                    $(Icon::$variant => include_str!(concat!("../assets/icons/", $file, ".svg"))),+
                }
            }
        }
    };
}

icons! {
    ChevronDown => "chevron-down",
    ChevronRight => "chevron-right",
    CircleAlert => "circle-alert",
    CircleCheck => "circle-check",
    Copy => "copy",
    LoaderCircle => "loader-circle",
    Minus => "minus",
    Play => "play",
    Plug => "plug",
    RefreshCw => "refresh-cw",
    Search => "search",
    Server => "server",
    Settings => "settings",
    Square => "square",
    Unplug => "unplug",
    X => "x",
}

/// The icon stroke width for `theme`: metric `icon.stroke_width` in px on
/// Lucide's 24 px grid, else [`DEFAULT_STROKE`].
pub fn stroke_width(theme: &Theme) -> f32 {
    theme
        .dictionary()
        .metrics
        .get("icon.stroke_width")
        .filter(|metric| metric.kind == ResolvedMetricKind::Px && metric.value > 0.0)
        .map_or(DEFAULT_STROKE, |metric| metric.value as f32)
}

/// `icon`'s SVG source with its stroke at `stroke` and coloured [`MASK`].
pub fn source(icon: Icon, stroke: f32) -> String {
    icon.svg()
        .replace("stroke=\"currentColor\"", &format!("stroke=\"{MASK}\""))
        .replace("stroke-width=\"2\"", &format!("stroke-width=\"{stroke}\""))
}

/// `icon` as an egui image `size` points square, stroked at `stroke` and
/// tinted `colour`.
pub fn image(icon: Icon, stroke: f32, size: f32, colour: Color32) -> Image<'static> {
    let uri = format!("bytes://lucide/{}@{stroke}.svg", icon.name());
    let bytes = source(icon, stroke).into_bytes();
    Image::new(ImageSource::Bytes { uri: Cow::Owned(uri), bytes: bytes.into() })
        .fit_to_exact_size(Vec2::splat(size))
        .tint(colour)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_an_unmodified_lucide_stroke_svg() {
        for icon in Icon::ALL {
            let svg = icon.svg();
            assert!(svg.contains("viewBox=\"0 0 24 24\""), "{}", icon.name());
            assert!(svg.contains("stroke=\"currentColor\""), "{}", icon.name());
            assert!(svg.contains("stroke-width=\"2\""), "{}", icon.name());
        }
    }

    #[test]
    fn stroke_and_colour_are_rewritten() {
        let svg = source(Icon::Play, 1.25);
        assert!(svg.contains("stroke-width=\"1.25\""));
        assert!(svg.contains(&format!("stroke=\"{MASK}\"")));
        assert!(!svg.contains("currentColor"));
    }

    #[test]
    fn the_shipped_design_uses_lucide_default_stroke() {
        assert_eq!(stroke_width(&Theme::embedded()), DEFAULT_STROKE);
    }
}
