// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The resolved accent colour, as a renderer draws it: one accessor for every
//! consumer (the toolkit's selection and accent fills, a desktop portal's
//! accent colour), so they cannot drift apart.
//!
//! The accent depends on the style's widgets:
//!
//! - chrome widgets draw the `accent` chrome role, after the design's own
//!   chrome mapping (an authored colour, or the role derived from the
//!   palette's pairs);
//! - pair widgets draw the `accent` pair's rendered surface.
//!
//! A chrome style whose design lacks the `accent` role falls back to the
//! pair, as a renderer does.

use crate::family::style::Widgets;
use crate::{LinearRgba, ResolvedDictionary, UnstampedResolvedDesign};

/// A colour as 8-bit sRGB draws it, each channel 0 to 1 (gamma-encoded
/// sRGB; alpha unpremultiplied).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SrgbColour {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

impl SrgbColour {
    fn from_linear(colour: LinearRgba) -> Self {
        let [red, green, blue, alpha] = colour.to_srgba8().map(|c| f64::from(c) / 255.0);
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    /// The 8-bit channels, `[r, g, b, a]`.
    pub fn to_srgba8(self) -> [u8; 4] {
        [self.red, self.green, self.blue, self.alpha].map(|c| (c * 255.0).round() as u8)
    }

    /// `#RRGGBBAA`, upper case: the form theme files are written in.
    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.to_srgba8();
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    }

    /// `#rrggbb` or `#rrggbbaa`, either case, `#` required: the settings
    /// wire grammar. Deliberately stricter than the legacy v0 theme reader
    /// ([`crate::parse_legacy_v0_hex_colour`]), which also takes short forms
    /// and an optional `#`.
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.strip_prefix('#')?;
        if !matches!(digits.len(), 6 | 8) || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |i: usize| {
            digits
                .get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .map(|byte| f64::from(byte) / 255.0)
        };
        Some(Self {
            red: channel(0)?,
            green: channel(2)?,
            blue: channel(4)?,
            alpha: if digits.len() == 8 { channel(6)? } else { 1.0 },
        })
    }
}

/// The accent `design` draws in its own style (its style family's selected
/// style; pair widgets when the design authors no style family). `None`
/// when the design has neither the chrome role nor the `accent` pair.
pub fn resolved_accent(design: &UnstampedResolvedDesign) -> Option<SrgbColour> {
    let dictionary = design.dictionary();
    let widgets = dictionary
        .style
        .map_or(Widgets::Pairs, |style| style.widgets);
    accent_for(dictionary, widgets)
}

/// The accent `dictionary` draws with `widgets`. A renderer that resolves a
/// style for a design without a style family passes that style's widgets.
pub fn accent_for(dictionary: &ResolvedDictionary, widgets: Widgets) -> Option<SrgbColour> {
    let chrome = match widgets {
        Widgets::Chrome => dictionary.chrome.as_ref().and_then(|c| c.get("accent")),
        Widgets::Pairs => None,
    };
    chrome
        .or_else(|| {
            dictionary
                .colours
                .pairs
                .get("accent")
                .map(|p| p.rendered_surface)
        })
        .map(SrgbColour::from_linear)
}
