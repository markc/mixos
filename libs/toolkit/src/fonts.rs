// SPDX-License-Identifier: MIT OR Apache-2.0
//! The embedded faces and how the theme's typography selects them.
//!
//! Weight is a design token: each typography role names a weight, and the
//! face installed for it is the static Inter instance of that weight
//! ([`inter_face`]). Changing the design's weights (for example to the light
//! profile, 300) changes the faces with no code change. egui cannot drive a
//! variable font's axes, so every weight is its own static file.
//!
//! Families installed on the context:
//!
//! | family | face |
//! |---|---|
//! | `Proportional` | Inter at the `ui` role's weight |
//! | [`MEDIUM`] | Inter at the `ui` weight + 100 (emphasis, selected rows) |
//! | [`HEADING`] | Inter at the `ui_display` role's weight |
//! | `Monospace` | JetBrains Mono |
//!
//! Each named family falls back to the proportional stack, so a missing glyph
//! still renders through egui's own fallbacks.

use crate::theme::Theme;
use design::TypographyRole;
use egui::{FontData, FontDefinitions, FontFamily, FontId};
use std::sync::Arc;

/// The named family for emphasised body text.
pub const MEDIUM: &str = "medium";

/// The named family for headings.
pub const HEADING: &str = "heading";

/// The body weight of the chrome schemes: Inter Regular.
pub const CHROME_WEIGHT: u16 = 400;

const INTER_LIGHT: &[u8] = include_bytes!("../assets/fonts/Inter-Light.ttf");
const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
const INTER_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
const JETBRAINS_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");

/// The embedded static Inter face nearest `weight` (CSS numbers): 300, 400,
/// 500 or 600, with lighter requests taking Light and heavier SemiBold.
pub fn inter_face(weight: u16) -> (&'static str, &'static [u8]) {
    match weight {
        0..=349 => ("Inter-Light", INTER_LIGHT),
        350..=449 => ("Inter-Regular", INTER_REGULAR),
        450..=549 => ("Inter-Medium", INTER_MEDIUM),
        _ => ("Inter-SemiBold", INTER_SEMIBOLD),
    }
}

/// The font definitions for `theme`.
pub fn definitions(theme: &Theme) -> FontDefinitions {
    let typography = theme.typography();
    let mut ui = design::active_typography(Some(typography), TypographyRole::Ui).weight;
    let mut display = design::active_typography(Some(typography), TypographyRole::UiDisplay).weight;
    // The chrome specification fixes its faces (§2.1): Inter Regular, with
    // Medium (window title, push buttons) and SemiBold (headings).
    if theme.style().faces == design::family::style::Faces::Fixed {
        (ui, display) = (CHROME_WEIGHT, CHROME_WEIGHT + 200);
    }

    let mut fonts = FontDefinitions::default();
    let mut add = |(name, bytes): (&'static str, &'static [u8])| {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
        name.to_owned()
    };
    let body = add(inter_face(ui));
    let medium = add(inter_face(ui.saturating_add(100)));
    let heading = add(inter_face(display));
    let mono = add(("JetBrainsMono-Regular", JETBRAINS_MONO));

    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, body);
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, mono);
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    for (family, primary) in [(MEDIUM, medium), (HEADING, heading)] {
        let mut stack = vec![primary];
        stack.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(family.into()), stack);
    }
    fonts
}

/// Install `theme`'s fonts on `ctx`.
pub fn install(ctx: &egui::Context, theme: &Theme) {
    ctx.set_fonts(definitions(theme));
}

/// Emphasised body text at `size`.
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MEDIUM.into()))
}

/// `font`, or the same size in the proportional family while `font`'s
/// family is not bound yet: fonts installed with [`install`] bind from the
/// next frame, and egui refuses an unbound family.
pub fn bound(ctx: &egui::Context, font: FontId) -> FontId {
    if ctx.fonts(|f| f.definitions().families.contains_key(&font.family)) {
        font
    } else {
        FontId::new(font.size, FontFamily::Proportional)
    }
}

/// Heading text at `size`.
pub fn heading(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(HEADING.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_select_their_static_faces() {
        assert_eq!(inter_face(300).0, "Inter-Light");
        assert_eq!(inter_face(400).0, "Inter-Regular");
        assert_eq!(inter_face(500).0, "Inter-Medium");
        assert_eq!(inter_face(600).0, "Inter-SemiBold");
        assert_eq!(inter_face(200).0, "Inter-Light");
        assert_eq!(inter_face(900).0, "Inter-SemiBold");
    }

    #[test]
    fn the_shipped_design_installs_regular_body_and_semibold_headings() {
        let fonts = definitions(&Theme::embedded());
        assert_eq!(fonts.families[&FontFamily::Proportional][0], "Inter-Regular");
        assert_eq!(fonts.families[&FontFamily::Name(MEDIUM.into())][0], "Inter-Medium");
        assert_eq!(fonts.families[&FontFamily::Name(HEADING.into())][0], "Inter-SemiBold");
        assert_eq!(fonts.families[&FontFamily::Monospace][0], "JetBrainsMono-Regular");
    }
}
