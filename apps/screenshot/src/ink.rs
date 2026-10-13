// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The annotation inks: six colours from design tokens. They are content,
//! not chrome, so they come from one fixed light design (revision one)
//! rather than the window's theme: an arrow drawn red stays the same red
//! whichever scheme or mode the window shows.
use design::DesignContext;

/// One ink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ink {
    /// The command id's last part (`annotate.colour.<name>`).
    pub name: &'static str,
    /// Its Fluent label key.
    pub label: &'static str,
    /// Straight sRGB with alpha.
    pub rgba: [u8; 4],
}

/// The inks' names, labels and the design token each comes from.
pub const INKS: [(&str, &str, &str); 6] = [
    ("red", "ink-red", "status.danger"),
    ("amber", "ink-amber", "status.warning"),
    ("green", "ink-green", "status.success"),
    ("teal", "ink-teal", "palette.accent.default"),
    ("black", "ink-black", "palette.foreground.default"),
    ("white", "ink-white", "palette.background.1"),
];

/// The inks, resolved from the fixed design.
pub fn inks() -> [Ink; 6] {
    let theme = toolkit::Theme::for_context(DesignContext::revision_one());
    let primitives = &theme.dictionary().colours.primitives;
    INKS.map(|(name, label, token)| {
        let rgba = primitives.get(token).map_or([0, 0, 0, 255], |c| {
            toolkit::style::colour(*c).to_srgba_unmultiplied()
        });
        Ink { name, label, rgba }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ink_resolves_to_an_opaque_distinct_colour() {
        let inks = inks();
        for ink in inks {
            assert_eq!(ink.rgba[3], 255, "{}", ink.name);
        }
        let mut colours: Vec<_> = inks.iter().map(|i| i.rgba).collect();
        colours.sort();
        colours.dedup();
        assert_eq!(colours.len(), inks.len(), "{inks:?}");
        // Red is mostly red, white is light, black is dark.
        let [r, g, b, _] = inks[0].rgba;
        assert!(r > g && r > b);
        assert!(inks[5].rgba[..3].iter().all(|c| *c > 200));
        assert!(inks[4].rgba[..3].iter().all(|c| *c < 80));
    }
}
