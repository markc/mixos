// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! What the GTK and Qt exporters write, from one derived theme (clean-room
//! daemon spec §2.2 and §2.3, as §6.3 and §6.4 apply them to MixOS desks):
//! libadwaita's named colours, and the three qt5ct/qt6ct palette groups of
//! 21 opaque colours each in Qt's role order. The exporters only format
//! these (the decision's ruling 1).

use std::collections::BTreeMap;

use serde::Serialize;

use super::DerivedTheme;
use super::primitives::{BLACK, WHITE, over, ramp, shift_lightness, with_alpha};
use crate::SrgbColour;

/// Qt's palette roles in the order a qt5ct/qt6ct colour scheme lists them.
pub const QT_ROLES: [&str; 21] = [
    "WindowText",
    "Button",
    "Light",
    "Midlight",
    "Dark",
    "Mid",
    "Text",
    "BrightText",
    "ButtonText",
    "Base",
    "Window",
    "Shadow",
    "Highlight",
    "HighlightedText",
    "Link",
    "LinkVisited",
    "AlternateBase",
    "NoRole",
    "ToolTipBase",
    "ToolTipText",
    "PlaceholderText",
];

const WINDOW_TEXT: usize = 0;
const BUTTON: usize = 1;
const TEXT: usize = 6;
const BUTTON_TEXT: usize = 8;
const BASE: usize = 9;
const WINDOW: usize = 10;
const HIGHLIGHTED_TEXT: usize = 13;
const LINK: usize = 14;
const LINK_VISITED: usize = 15;
const ALTERNATE_BASE: usize = 16;
const TOOL_TIP_BASE: usize = 18;
const TOOL_TIP_TEXT: usize = 19;
const PLACEHOLDER_TEXT: usize = 20;

/// The toolkit exports of one mode.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Toolkit {
    /// libadwaita's named colours, for `@define-color` in `gtk.css`.
    pub gtk: BTreeMap<&'static str, SrgbColour>,
    /// The qt5ct/qt6ct colour scheme, each group in [`QT_ROLES`] order.
    pub qt: QtPalette,
}

/// The three palette groups, every colour flattened over its group's
/// `Base` so none is translucent.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QtPalette {
    pub active: [SrgbColour; 21],
    pub inactive: [SrgbColour; 21],
    pub disabled: [SrgbColour; 21],
}

/// `a` mixed `p` of the way towards `b`, channel by channel (alpha too) on
/// gamma-encoded sRGB.
fn mix(a: SrgbColour, b: SrgbColour, p: f64) -> SrgbColour {
    let lerp = |x: f64, y: f64| x + (y - x) * p;
    SrgbColour {
        red: lerp(a.red, b.red),
        green: lerp(a.green, b.green),
        blue: lerp(a.blue, b.blue),
        alpha: lerp(a.alpha, b.alpha),
    }
}

/// Lightened by 0.2 and 0.1, as is, darkened by 0.1 and 0.2 (OKLab).
fn five(seed: SrgbColour) -> [SrgbColour; 5] {
    [0.2, 0.1, 0.0, -0.1, -0.2].map(|delta| shift_lightness(seed, delta))
}

impl Toolkit {
    pub fn of(theme: &DerivedTheme) -> Self {
        Self {
            gtk: gtk(theme),
            qt: qt(theme),
        }
    }
}

fn gtk(t: &DerivedTheme) -> BTreeMap<&'static str, SrgbColour> {
    let dark = t.is_dark;
    let (sidebar_shade, shade, backdrop) = if dark {
        (0.08, 0.32, 0.08)
    } else {
        (0.32, 0.08, 0.32)
    };
    let bg = &t.background;
    let component = &bg.component;
    let mut out: BTreeMap<&'static str, SrgbColour> = BTreeMap::new();
    let mut put = |name: &'static str, colour: SrgbColour| {
        out.insert(name, colour);
    };
    put("window_bg_color", bg.base);
    put("window_fg_color", bg.on);
    put("view_bg_color", t.primary.base);
    put("view_fg_color", t.primary.on);
    put("headerbar_bg_color", bg.base);
    put("headerbar_fg_color", bg.on);
    // The reference writes this name with "_color" doubled, which no
    // libadwaita stylesheet reads; MixOS writes the name libadwaita uses.
    put("headerbar_border_color", bg.divider);
    put("headerbar_backdrop_color", bg.base);
    for (prefix, container) in [("sidebar", &t.primary), ("secondary_sidebar", &t.secondary)] {
        let names: [&'static str; 4] = match prefix {
            "sidebar" => [
                "sidebar_bg_color",
                "sidebar_fg_color",
                "sidebar_shade_color",
                "sidebar_backdrop_color",
            ],
            _ => [
                "secondary_sidebar_bg_color",
                "secondary_sidebar_fg_color",
                "secondary_sidebar_shade_color",
                "secondary_sidebar_backdrop_color",
            ],
        };
        put(names[0], container.base);
        put(names[1], container.on);
        put(names[2], with_alpha(BLACK, sidebar_shade));
        put(names[3], over(with_alpha(WHITE, backdrop), container.base));
    }
    for (bg_name, fg_name) in [
        ("card_bg_color", "card_fg_color"),
        ("thumbnail_bg_color", "thumbnail_fg_color"),
        ("popover_bg_color", "popover_fg_color"),
    ] {
        put(bg_name, component.base);
        put(fg_name, component.on);
    }
    put("dialog_bg_color", t.primary.base);
    put("dialog_fg_color", t.primary.on);
    put("shade_color", with_alpha(BLACK, shade));
    put("scrollbar_outline_color", with_alpha(bg.base, 0.5));
    for (names, family) in [
        (
            ["accent_color", "accent_bg_color", "accent_fg_color"],
            &t.accent,
        ),
        (
            [
                "destructive_color",
                "destructive_bg_color",
                "destructive_fg_color",
            ],
            &t.destructive,
        ),
        (
            ["warning_color", "warning_bg_color", "warning_fg_color"],
            &t.warning,
        ),
        (
            ["success_color", "success_bg_color", "success_fg_color"],
            &t.success,
        ),
        (
            ["error_color", "error_bg_color", "error_fg_color"],
            &t.destructive,
        ),
    ] {
        put(names[0], family.base);
        put(names[1], family.base);
        put(names[2], family.on);
    }
    let p = &t.palette;
    for (names, seed) in [
        (
            ["blue_1", "blue_2", "blue_3", "blue_4", "blue_5"],
            p.accent_blue,
        ),
        (
            ["green_1", "green_2", "green_3", "green_4", "green_5"],
            p.accent_green,
        ),
        (
            ["yellow_1", "yellow_2", "yellow_3", "yellow_4", "yellow_5"],
            p.accent_yellow,
        ),
        (["red_1", "red_2", "red_3", "red_4", "red_5"], p.accent_red),
        (
            ["orange_1", "orange_2", "orange_3", "orange_4", "orange_5"],
            p.ext_orange,
        ),
        (
            ["purple_1", "purple_2", "purple_3", "purple_4", "purple_5"],
            p.ext_purple,
        ),
    ] {
        for (name, colour) in names.into_iter().zip(five(seed)) {
            put(name, colour);
        }
    }
    // libadwaita's names with libadwaita's meaning, white to black: the
    // reference writes 0-based names in ramp order, which no stylesheet
    // reads (Fable, daemon spec §2.2 note).
    let steps = ramp::<10>(p.neutral[5]);
    let names = [
        "dark_5", "dark_4", "dark_3", "dark_2", "dark_1", "light_5", "light_4", "light_3",
        "light_2", "light_1",
    ];
    for (name, colour) in names.into_iter().zip(steps) {
        put(name, colour);
    }
    out
}

fn qt(t: &DerivedTheme) -> QtPalette {
    let bg = &t.background;
    let component = &bg.component;
    let (lightest, darkest) = if t.is_dark {
        (bg.on, bg.base)
    } else {
        (bg.base, bg.on)
    };
    let button = &t.button;
    let link = &t.link_button;
    let active: [SrgbColour; 21] = [
        bg.on,
        button.base,
        mix(button.base, lightest, 0.10),
        mix(button.base, lightest, 0.05),
        mix(button.base, darkest, 0.10),
        mix(button.base, darkest, 0.05),
        component.on,
        lightest,
        button.on,
        component.base,
        bg.base,
        darkest,
        // Highlight and HighlightedText are swapped on purpose, as the
        // reference does, for menu-bar contrast.
        component.selected_text,
        component.selected,
        link.on,
        mix(link.on, t.secondary.component.base, 0.2),
        mix(bg.base, t.accent.base, 0.05),
        component.disabled,
        component.base,
        component.on,
        with_alpha(component.on, 0.5),
    ];
    let mut inactive = active;
    for role in [WINDOW_TEXT, TEXT, HIGHLIGHTED_TEXT, TOOL_TIP_TEXT] {
        inactive[role] = with_alpha(inactive[role], 0.8);
    }
    let mut disabled = inactive;
    disabled[BUTTON] = button.disabled;
    disabled[TEXT] = component.on_disabled;
    disabled[BUTTON_TEXT] = button.on_disabled;
    disabled[BASE] = component.disabled;
    disabled[HIGHLIGHTED_TEXT] = with_alpha(disabled[HIGHLIGHTED_TEXT], 0.5);
    disabled[LINK] = link.on_disabled;
    disabled[LINK_VISITED] = mix(link.on_disabled, t.secondary.component.disabled, 0.2);
    disabled[ALTERNATE_BASE] = mix(bg.base, t.accent.disabled, 0.05);
    disabled[TOOL_TIP_BASE] = component.disabled;
    disabled[TOOL_TIP_TEXT] = component.on_disabled;
    disabled[PLACEHOLDER_TEXT] = with_alpha(component.on_disabled, 0.5);
    QtPalette {
        active: flatten(active),
        inactive: flatten(inactive),
        disabled: flatten(disabled),
    }
}

/// Every colour of a group over the group's `Base`, which itself is first
/// flattened over the group's `Window` (and that over black), so the whole
/// group is opaque.
fn flatten(group: [SrgbColour; 21]) -> [SrgbColour; 21] {
    let window = over(group[WINDOW], BLACK);
    let base = over(group[BASE], window);
    group.map(|colour| over(colour, base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::{FrostedGlass, Seeds, derive};
    use crate::{Contrast, Mode};

    fn theme(mode: Mode) -> DerivedTheme {
        derive(
            &Seeds::default(),
            mode,
            Contrast::Normal,
            &FrostedGlass::default(),
        )
    }

    #[test]
    fn gtk_names_cover_the_libadwaita_set() {
        let gtk = Toolkit::of(&theme(Mode::Dark)).gtk;
        assert_eq!(gtk.len(), 81);
        let t = theme(Mode::Dark);
        assert_eq!(gtk["window_bg_color"], t.background.base);
        assert_eq!(gtk["accent_bg_color"], t.accent.base);
        assert_eq!(gtk["accent_fg_color"], t.accent.on);
        assert_eq!(gtk["error_bg_color"], t.destructive.base);
        assert_eq!(gtk["blue_3"].to_hex(), t.palette.accent_blue.to_hex());
        assert_eq!(gtk["shade_color"].alpha, 0.32);
        assert_eq!(gtk["sidebar_shade_color"].alpha, 0.08);
        assert!(!gtk.contains_key("headerbar_border_color_color"));
    }

    #[test]
    fn five_steps_get_lighter_then_darker() {
        let gtk = Toolkit::of(&theme(Mode::Light)).gtk;
        let l = |name: &str| super::super::primitives::lightness(gtk[name]);
        assert!(l("blue_1") > l("blue_2") && l("blue_2") > l("blue_3"));
        assert!(l("blue_3") > l("blue_4") && l("blue_4") > l("blue_5"));
        // light_1 is white and dark_5 black, as libadwaita names them.
        assert!(l("light_1") > 0.99 && l("light_1") > l("light_5"));
        assert!(l("light_5") > l("dark_1") && l("dark_1") > l("dark_5"));
        assert!(l("dark_5") < 0.01);
        assert!(!gtk.contains_key("light_0") && !gtk.contains_key("dark_0"));
    }

    #[test]
    fn qt_groups_are_opaque_and_in_role_order() {
        for mode in [Mode::Dark, Mode::Light] {
            let t = theme(mode);
            let qt = Toolkit::of(&t).qt;
            for group in [qt.active, qt.inactive, qt.disabled] {
                assert!(group.iter().all(|c| c.alpha == 1.0));
            }
            assert_eq!(
                qt.active[WINDOW].to_hex(),
                over(t.background.base, BLACK).to_hex()
            );
            assert_eq!(QT_ROLES[HIGHLIGHTED_TEXT], "HighlightedText");
            assert_eq!(QT_ROLES[PLACEHOLDER_TEXT], "PlaceholderText");
        }
    }

    #[test]
    fn inactive_and_disabled_derive_from_active() {
        let qt = Toolkit::of(&theme(Mode::Dark)).qt;
        // Window and the bases do not change between active and inactive.
        assert_eq!(qt.active[WINDOW], qt.inactive[WINDOW]);
        assert_eq!(qt.active[BUTTON], qt.inactive[BUTTON]);
        // Text dims in the inactive group, and further when disabled.
        assert_ne!(qt.active[TEXT], qt.inactive[TEXT]);
        assert_ne!(qt.inactive[TEXT], qt.disabled[TEXT]);
    }

    #[test]
    fn mix_is_a_channel_lerp() {
        let m = mix(BLACK, WHITE, 0.25);
        assert_eq!((m.red, m.green, m.blue, m.alpha), (0.25, 0.25, 0.25, 1.0));
    }
}
