// SPDX-License-Identifier: MIT OR Apache-2.0
//! The editor's colours, from the MixOS theme.
//!
//! Surfaces and text come from the toolkit's chrome roles, the same ones
//! every chrome component draws with, so the editor sits in any scheme the
//! way a text field does: the `field` surface, `text`, `text_dim` and
//! `text_faint`, the `hover` row and the scheme's selection.
//!
//! The design has no `syntax.*` tokens yet, so highlight classes map onto
//! roles it has: keywords and headings take the accent, types, functions and
//! links its hover shade, strings and insertions the success status, numbers
//! and constants the warning role, deletions and invalid text the danger
//! role, comments and punctuation the dim text. Every class keeps at least
//! [`MIN_HL_CONTRAST`] against the background, pulled toward the text colour
//! where it would not.

use design::{LinearRgba, contrast_ratio};
use editor_model::highlight::HlClass;
use egui::{Color32, Rgba};
use toolkit::Theme;
use toolkit::chrome::Chrome;
use toolkit::style::colour;

/// Highlight classes, [`HlClass::Plain`] to [`HlClass::Invalid`].
pub const HL_CLASSES: usize = HlClass::Invalid as usize + 1;

/// Minimum contrast of every highlight colour against the background.
pub const MIN_HL_CONTRAST: f64 = 3.0;

/// How strongly tints (selection, find matches, other origins) cover the
/// background.
const SELECTION_ALPHA: f32 = 0.45;
pub(crate) const MATCH_ALPHA: f32 = 0.22;
pub(crate) const REMOTE_ALPHA: f32 = 0.18;

#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub background: Color32,
    pub text: Color32,
    pub gutter_text: Color32,
    pub current_line: Color32,
    pub selection: Color32,
    pub caret: Color32,
    /// Lines in the gutter: the separator and the current line number.
    pub separator: Color32,
    /// Other people's carets and changes.
    pub human_other: Color32,
    /// Agents' carets and changes.
    pub agent: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub note: Color32,
    pub highlight: [Color32; HL_CLASSES],
}

/// The roles a highlight class can take, before the legibility pull.
struct Roles {
    text: Color32,
    dim: Color32,
    accent: Color32,
    accent_hover: Color32,
    success: Color32,
    warning: Color32,
    danger: Color32,
}

impl Palette {
    pub fn hl(&self, class: HlClass) -> Color32 {
        self.highlight[class as usize]
    }

    /// The editor colours of `theme`.
    pub fn from_theme(theme: &Theme) -> Self {
        let chrome = Chrome::for_theme(theme);
        let c = &chrome.palette;
        let d = theme.dictionary();
        let prim =
            |name: &str, or: Color32| d.colours.primitives.get(name).map_or(or, |v| colour(*v));
        // The hue schemes read their roles back from egui's visuals, where
        // dim text can equal text; the design's muted foreground then serves.
        let dim = if c.text_dim == c.text {
            prim("palette.foreground.muted", c.text)
        } else {
            c.text_dim
        };
        let roles = Roles {
            text: c.text,
            dim,
            accent: c.accent,
            accent_hover: prim("palette.accent.hover", c.accent),
            success: prim("status.success", c.text),
            warning: c.warning,
            danger: c.danger,
        };
        let background = c.field;
        let mut highlight = [c.text; HL_CLASSES];
        for (slot, class) in highlight.iter_mut().zip(ALL_CLASSES) {
            *slot = legible(class_role(&roles, class), c.text, background);
        }
        Self {
            background,
            text: c.text,
            gutter_text: legible(c.text_faint, c.text, background),
            current_line: c.hover,
            selection: selection(c.selection(&chrome.style), c.accent),
            caret: c.text,
            separator: c.separator,
            human_other: c.accent,
            agent: c.warning,
            error: c.danger,
            warning: c.warning,
            note: c.text_dim,
            highlight,
        }
    }
}

/// The selection under coloured text: a scheme that selects with its soft
/// accent keeps it; the full accent is faded so the text stays legible.
fn selection(scheme: Color32, accent: Color32) -> Color32 {
    if scheme == accent {
        scheme.gamma_multiply(SELECTION_ALPHA)
    } else {
        scheme
    }
}

fn class_role(r: &Roles, class: HlClass) -> Color32 {
    match class {
        HlClass::Plain | HlClass::Variable | HlClass::Operator => r.text,
        HlClass::Comment | HlClass::Punctuation | HlClass::Meta => r.dim,
        HlClass::Keyword | HlClass::Heading => r.accent,
        HlClass::Type | HlClass::Function | HlClass::Link => r.accent_hover,
        HlClass::String | HlClass::Inserted => r.success,
        HlClass::Number | HlClass::Constant => r.warning,
        HlClass::Deleted | HlClass::Invalid => r.danger,
    }
}

/// Every [`HlClass`], in discriminant order.
const ALL_CLASSES: [HlClass; HL_CLASSES] = [
    HlClass::Plain,
    HlClass::Comment,
    HlClass::Keyword,
    HlClass::String,
    HlClass::Number,
    HlClass::Constant,
    HlClass::Type,
    HlClass::Function,
    HlClass::Variable,
    HlClass::Operator,
    HlClass::Punctuation,
    HlClass::Meta,
    HlClass::Inserted,
    HlClass::Deleted,
    HlClass::Heading,
    HlClass::Link,
    HlClass::Invalid,
];

fn linear(c: Color32) -> LinearRgba {
    let l = Rgba::from(c.to_opaque());
    LinearRgba {
        red: f64::from(l.r()),
        green: f64::from(l.g()),
        blue: f64::from(l.b()),
        alpha: 1.0,
    }
}

/// The contrast ratio of two colours as drawn (opaque).
pub fn contrast(a: Color32, b: Color32) -> f64 {
    contrast_ratio(linear(a), linear(b))
}

/// `candidate` if it reaches [`MIN_HL_CONTRAST`] on `background`; otherwise
/// the nearest mix toward `text` that does, and `text` as the last resort.
fn legible(candidate: Color32, text: Color32, background: Color32) -> Color32 {
    if contrast(candidate, background) >= MIN_HL_CONTRAST {
        return candidate;
    }
    for step in 1..10 {
        let mixed = candidate.lerp_to_gamma(text, step as f32 / 10.0);
        if contrast(mixed, background) >= MIN_HL_CONTRAST {
            return mixed;
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use design::{DesignContext, Mode, Scheme};

    #[test]
    fn classes_are_in_discriminant_order() {
        for (i, c) in ALL_CLASSES.iter().enumerate() {
            assert_eq!(*c as usize, i);
        }
    }

    #[test]
    fn every_class_is_legible_and_keywords_stand_out_in_every_scheme() {
        let schemes = [
            Scheme::Pro,
            Scheme::Studio,
            Scheme::Classic,
            Scheme::Ocean,
            Scheme::Mono,
        ];
        for scheme in schemes {
            for mode in [Mode::Light, Mode::Dark] {
                let theme = Theme::for_context(DesignContext {
                    scheme,
                    mode,
                    ..DesignContext::default()
                });
                let p = Palette::from_theme(&theme);
                for class in ALL_CLASSES {
                    let c = p.hl(class);
                    assert!(
                        contrast(c, p.background) >= MIN_HL_CONTRAST - 1e-6 || c == p.text,
                        "{scheme:?} {mode:?} {class:?}"
                    );
                }
                if scheme != Scheme::Mono {
                    assert_ne!(
                        p.hl(HlClass::Keyword),
                        p.text,
                        "{scheme:?} {mode:?}: keywords"
                    );
                    assert_ne!(
                        p.hl(HlClass::Comment),
                        p.text,
                        "{scheme:?} {mode:?}: comments"
                    );
                }
            }
        }
    }
}
