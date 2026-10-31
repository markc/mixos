// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The COSMIC-model theme derivation: from the builder's seeds, one palette
//! and the frosted settings to the full derived theme of one mode (six
//! containers, the button family, accent text and shade).
//!
//! This is the clean-room theme-model spec §4 as written, including the
//! reference's quirks the worked numbers depend on: `Idx`'s factor of 100
//! with its re-index shift, the non-standard `Over` operator, the overlay
//! cross-overs between containers, and the primary container's texts
//! taking the container alpha even with no frosted flag set. Geometry
//! (spacing, radii, gaps, hint) is not derived; it is copied through by the
//! caller from [`crate::cosmic_tables`].
//!
//! Colours are gamma-encoded sRGB with straight alpha ([`SrgbColour`]).

pub(crate) mod primitives;
#[cfg(test)]
mod tests;

use primitives::{
    BLACK, Ramp, TRANSPARENT, WHITE, contrast, idx, luminance, over, ramp, rehue, small, surface,
    text, with_alpha,
};

use crate::cosmic_tables::{AlphaMap, BasePalette, BlurStrength, DEFAULT_ALPHA_MAP};
use crate::{Contrast, Mode, SrgbColour};

/// The builder's colour inputs. `None` means "derive it".
///
/// The seeds marked opaque have their alpha forced to 1 by the derivation
/// (a ramp inherits its seed's alpha, so a translucent accent or tint would
/// otherwise reach every control and text). The three backgrounds keep
/// theirs.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct Seeds {
    /// Opaque; `None` is the palette's `accent_blue`.
    pub accent: Option<SrgbColour>,
    /// The window background, with alpha; `None` is the palette's `gray_1`.
    pub bg_color: Option<SrgbColour>,
    /// With alpha; `None` derives it from the background.
    pub primary_container_bg: Option<SrgbColour>,
    /// With alpha; `None` derives it from the background.
    pub secondary_container_bg: Option<SrgbColour>,
    /// Opaque; tints the control and neutral ramps.
    pub neutral_tint: Option<SrgbColour>,
    /// Opaque; when set, every text colour comes from its ramp.
    pub text_tint: Option<SrgbColour>,
    /// Opaque; `None` is the palette's `bright_green`.
    pub success: Option<SrgbColour>,
    /// Opaque; `None` is the palette's `bright_orange`.
    pub warning: Option<SrgbColour>,
    /// Opaque; `None` is the palette's `bright_red`.
    pub destructive: Option<SrgbColour>,
    /// Opaque; not derived from, only copied through (`None`: the
    /// compositor uses the accent).
    pub window_hint: Option<SrgbColour>,
}

impl Seeds {
    /// These seeds with every opaque-only colour forced to alpha 1.
    fn normalised(&self) -> Self {
        let opaque = |seed: Option<SrgbColour>| seed.map(|colour| with_alpha(colour, 1.0));
        Self {
            accent: opaque(self.accent),
            neutral_tint: opaque(self.neutral_tint),
            text_tint: opaque(self.text_tint),
            success: opaque(self.success),
            warning: opaque(self.warning),
            destructive: opaque(self.destructive),
            window_hint: opaque(self.window_hint),
            ..*self
        }
    }
}

/// The frosted-glass inputs.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct FrostedGlass {
    pub strength: BlurStrength,
    pub alpha_map: AlphaMap,
    pub windows: bool,
    pub system_interface: bool,
    pub panel: bool,
    pub applets: bool,
    /// Copied through; it does not count towards [`Self::actual_alpha`].
    pub maximized_apps: bool,
}

impl Default for FrostedGlass {
    fn default() -> Self {
        Self {
            strength: BlurStrength::default(),
            alpha_map: DEFAULT_ALPHA_MAP,
            windows: false,
            system_interface: false,
            panel: false,
            applets: false,
            maximized_apps: false,
        }
    }
}

impl FrostedGlass {
    /// The alpha-map entry of the chosen strength.
    pub fn container_alpha(&self) -> f64 {
        self.alpha_map[self.strength.index()]
    }

    /// [`Self::container_alpha`] when any of windows, system interface,
    /// panel or applets is frosted, else 1.
    pub fn actual_alpha(&self) -> f64 {
        if self.windows || self.system_interface || self.panel || self.applets {
            self.container_alpha()
        } else {
            1.0
        }
    }
}

/// One component's twelve colours. Serialised with the reference's field
/// names, each colour as `#RRGGBBAA`.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Component {
    pub base: SrgbColour,
    pub hover: SrgbColour,
    pub pressed: SrgbColour,
    pub selected: SrgbColour,
    pub selected_text: SrgbColour,
    pub focus: SrgbColour,
    pub divider: SrgbColour,
    pub on: SrgbColour,
    pub disabled: SrgbColour,
    pub on_disabled: SrgbColour,
    pub border: SrgbColour,
    pub disabled_border: SrgbColour,
}

/// One container: its surface, the component drawn on it, and its texts.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Container {
    pub base: SrgbColour,
    pub component: Component,
    pub divider: SrgbColour,
    pub on: SrgbColour,
    pub small_widget: SrgbColour,
}

/// The derived theme of one mode: every colour the derivation produces,
/// plus its copied-through colour and frosted inputs. Geometry (spacing,
/// radii, gaps, hint width) and the palette's name are not derived and
/// belong to the caller (stage S2 assembles the full theme record).
///
/// The spec's §4.7 consumer accessors that are plain field reads are the
/// fields themselves: a colour's `base` (accent, success, warning,
/// destructive, and the background, primary and secondary containers), its
/// `on`, `component.base`, and `button.base` / `button.on`. The ones with a
/// rule are methods: [`Self::control`], [`Self::accent_text_colour`],
/// [`Self::small_widget_divider`], [`Self::window_header_background`].
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct DerivedTheme {
    /// The palette derived from, with its kind (mode and contrast).
    pub palette: BasePalette,
    pub is_dark: bool,
    pub is_high_contrast: bool,
    pub background: Container,
    pub primary: Container,
    pub secondary: Container,
    pub transparent_background: Container,
    pub transparent_primary: Container,
    pub transparent_secondary: Container,
    pub accent: Component,
    pub accent_button: Component,
    pub button: Component,
    pub destructive: Component,
    pub destructive_button: Component,
    pub icon_button: Component,
    pub link_button: Component,
    pub list_button: Component,
    pub success: Component,
    pub success_button: Component,
    pub text_button: Component,
    pub warning: Component,
    pub warning_button: Component,
    /// The accent lifted for legibility on the primary container, when the
    /// accent itself is under 4:1 there.
    pub accent_text: Option<SrgbColour>,
    pub shade: SrgbColour,
    /// The builder's `neutral_tint`, copied through.
    pub control_tint: Option<SrgbColour>,
    pub text_tint: Option<SrgbColour>,
    pub window_hint: Option<SrgbColour>,
    pub frosted: FrostedGlass,
}

impl DerivedTheme {
    /// Control colour `k` (0..=10): the palette's `neutral_k`, re-hued with
    /// the control tint's OKLCh hue and chroma when one is set.
    pub fn control(&self, k: usize) -> Option<SrgbColour> {
        let neutral = *self.palette.neutral.get(k)?;
        Some(
            self.control_tint
                .map_or(neutral, |tint| rehue(neutral, tint)),
        )
    }

    /// The accent text colour: `accent_text` when set, else the accent.
    pub fn accent_text_colour(&self) -> SrgbColour {
        self.accent_text.unwrap_or(self.accent.base)
    }

    /// The small widget divider: the palette's `neutral_9` at alpha 0.2.
    pub fn small_widget_divider(&self) -> SrgbColour {
        with_alpha(self.palette.neutral[9], 0.2)
    }

    /// The window header background: the background container's base.
    pub fn window_header_background(&self) -> SrgbColour {
        self.background.base
    }
}

/// Derives the theme for `mode` and `contrast` from the built-in palette.
pub fn derive(
    seeds: &Seeds,
    mode: Mode,
    contrast: Contrast,
    frosted: &FrostedGlass,
) -> DerivedTheme {
    derive_with_palette(&BasePalette::builtin(mode, contrast), seeds, frosted)
}

/// Neutral(base, accent, on, hoverOverlay, pressedOverlay, hc, border).
fn neutral(
    base: SrgbColour,
    accent: SrgbColour,
    on: SrgbColour,
    hover_overlay: SrgbColour,
    pressed_overlay: SrgbColour,
    hc: bool,
    border: SrgbColour,
) -> Component {
    let state = |overlay| {
        if base.alpha < 0.001 {
            overlay
        } else {
            over(overlay, base)
        }
    };
    let hover = state(hover_overlay);
    Component {
        base,
        hover,
        pressed: state(pressed_overlay),
        selected: hover,
        selected_text: accent,
        focus: accent,
        divider: with_alpha(on, if hc { 0.65 } else { 0.2 }),
        on,
        disabled: with_alpha(base, base.alpha / 2.0),
        on_disabled: with_alpha(on, 0.65),
        border,
        disabled_border: with_alpha(border, border.alpha / 2.0),
    }
}

/// Coloured(base, neutral, accent, hoverOverlay, pressedOverlay): the
/// accent, success, warning and destructive components.
fn coloured(
    base: SrgbColour,
    neutral: SrgbColour,
    accent: SrgbColour,
    hover_overlay: SrgbColour,
    pressed_overlay: SrgbColour,
) -> Component {
    let hover = over(hover_overlay, base);
    let neutral = with_alpha(neutral, 1.0);
    Component {
        base,
        hover,
        pressed: over(pressed_overlay, base),
        selected: hover,
        selected_text: accent,
        focus: accent,
        divider: neutral,
        on: neutral,
        disabled: over(with_alpha(base, base.alpha / 2.0), base),
        on_disabled: over(with_alpha(neutral, 0.5), base),
        border: base,
        disabled_border: with_alpha(base, base.alpha / 2.0),
    }
}

/// ColouredButton: [`coloured`] with its own text colour.
fn coloured_button(
    base: SrgbColour,
    overlay: SrgbColour,
    on_button: SrgbColour,
    accent: SrgbColour,
    hover_overlay: SrgbColour,
    pressed_overlay: SrgbColour,
) -> Component {
    Component {
        on: on_button,
        on_disabled: with_alpha(on_button, 0.5),
        ..coloured(base, overlay, accent, hover_overlay, pressed_overlay)
    }
}

/// Container(component, base, on, small, hc).
fn container(
    component: Component,
    base: SrgbColour,
    on: SrgbColour,
    small: SrgbColour,
    hc: bool,
) -> Container {
    Container {
        base,
        component,
        divider: over(with_alpha(on, if hc { 0.5 } else { 0.2 }), base),
        on,
        small_widget: with_alpha(small, 0.25),
    }
}

/// §4.2 step 14 for `mode` and `contrast`: the accent lifted for legibility
/// against `bg` and `primary_base`, or `None` when the accent already reads.
/// The accent axis applies the same rule under an authored scheme.
pub(crate) fn accent_text_for(
    accent: SrgbColour,
    bg: SrgbColour,
    primary_base: SrgbColour,
    mode: Mode,
    contrast: Contrast,
) -> Option<SrgbColour> {
    accent_text(
        with_alpha(accent, 1.0),
        bg,
        primary_base,
        mode == Mode::Dark,
        contrast == Contrast::High,
    )
}

/// §4.2 step 14: the accent text colour, the only contrast check in the
/// model. Dark themes test against the primary base, light ones against
/// the darker of background and primary base.
fn accent_text(
    accent: SrgbColour,
    bg: SrgbColour,
    primary_base: SrgbColour,
    dark: bool,
    hc: bool,
) -> Option<SrgbColour> {
    let (against, fallback) = if dark {
        (primary_base, WHITE)
    } else if luminance(primary_base) < luminance(bg) {
        (primary_base, BLACK)
    } else {
        (bg, BLACK)
    };
    if contrast(against, accent) >= 4.0 {
        return None;
    }
    let steps = if hc { 60 } else { 50 };
    let candidate = surface(idx(against), steps, &ramp::<100>(accent), dark, fallback);
    Some(if contrast(against, candidate) < 4.0 {
        fallback
    } else {
        candidate
    })
}

/// Derives the theme from an explicit palette (a builder may carry an
/// imported one). The palette's kind decides dark and high contrast.
pub fn derive_with_palette(
    palette: &BasePalette,
    seeds: &Seeds,
    frosted: &FrostedGlass,
) -> DerivedTheme {
    let p = palette;
    let seeds = &seeds.normalised();
    // 1. Alphas.
    let container_alpha = frosted.container_alpha();
    let actual_alpha = frosted.actual_alpha();
    // 2. Flags.
    let dark = p.kind.is_dark();
    let hc = p.kind.is_high_contrast();
    // 3. Seeds.
    let accent = seeds.accent.unwrap_or(p.accent_blue);
    let success = seeds.success.unwrap_or(p.bright_green);
    let warning = seeds.warning.unwrap_or(p.bright_orange);
    let destructive = seeds.destructive.unwrap_or(p.bright_red);
    // 4. Text ramp.
    let text_ramp = seeds.text_tint.map(ramp::<100>);
    let tint: Option<&Ramp> = text_ramp.as_ref();
    // 5. Control ramp, black-ish first in dark and white-ish first in light.
    let mut controls = ramp::<11>(seeds.neutral_tint.unwrap_or(p.neutral[2]));
    if !dark {
        controls.reverse();
    }
    let body = controls[8];
    // 6. Neutral ramp.
    let neutral_ramp = ramp::<100>(seeds.neutral_tint.unwrap_or(BLACK));
    // 7. Background.
    let bg = seeds.bg_color.unwrap_or(p.gray_1);
    let bg_ramp = ramp::<100>(bg);
    let bg_idx = idx(bg);
    // 8. Transparent background.
    let tbg = with_alpha(bg, container_alpha);
    let t_ramp = ramp::<100>(tbg);
    // 9. Overlays. "Chosen from X": controls[10] when X < 91, else
    // controls[0]. Step 9's hover overlay is replaced at step 12 before
    // anything reads it, so only its pressed overlay is kept.
    let chosen = |index: usize| {
        if index < 91 {
            controls[10]
        } else {
            controls[0]
        }
    };
    let bg_pressed_overlay = with_alpha(chosen(bg_idx), 0.2);
    let button_bg = with_alpha(controls[7], 0.25);
    let button_hover = with_alpha(controls[5], 0.2);
    let button_pressed = with_alpha(controls[2], 0.5);
    // 10. Background component.
    let bg_comp = surface(bg_idx, 8, &bg_ramp, dark, p.neutral[2]);
    let on_bg_comp = text(idx(bg_comp), &bg_ramp, tint, actual_alpha);
    // 11. Transparent background component; its text is opaque (bgRamp).
    let tbg_comp = surface(bg_idx, 8, &t_ramp, dark, p.neutral[2]);
    let on_tbg_comp = text(idx(tbg_comp), &bg_ramp, tint, actual_alpha);
    // 12. Primary container; its texts use the container alpha.
    let primary_default = surface(bg_idx, 5, &bg_ramp, dark, controls[1]);
    let p_base = seeds.primary_container_bg.unwrap_or(primary_default);
    let p_ramp = ramp::<100>(p_base);
    let p_idx = idx(p_base);
    let p_comp = surface(p_idx, 6, &p_ramp, dark, controls[3]);
    let hover_overlay = with_alpha(chosen(p_idx), 0.1);
    let p_pressed = with_alpha(chosen(p_idx), 0.2);
    let primary = container(
        neutral(
            p_comp,
            accent,
            text(idx(p_comp), &p_ramp, tint, container_alpha),
            hover_overlay,
            p_pressed,
            hc,
            body,
        ),
        p_base,
        text(p_idx, &p_ramp, tint, container_alpha),
        small(p_idx, &neutral_ramp, controls[6]),
        hc,
    );
    // 13. Transparent primary.
    let tp_alpha = (container_alpha + if dark { 0.3 } else { 0.25 }).min(1.0);
    let tp_base = with_alpha(p_base, tp_alpha);
    let tp_ramp = ramp::<100>(tp_base);
    let tp_idx = idx(tp_base);
    let tp_comp = surface(tp_idx, 6, &tp_ramp, dark, controls[3]);
    let transparent_primary = container(
        neutral(
            tp_comp,
            accent,
            text(idx(tp_comp), &bg_ramp, tint, actual_alpha),
            TRANSPARENT,
            TRANSPARENT,
            hc,
            body,
        ),
        tp_base,
        text(tp_idx, &bg_ramp, tint, actual_alpha),
        small(tp_idx, &neutral_ramp, controls[6]),
        hc,
    );
    // 14. Accent text.
    let accent_text = accent_text(accent, bg, p_base, dark, hc);
    // 15. Shade.
    let shade = with_alpha(BLACK, if dark { 0.32 } else { 0.08 });
    // 16. Background container: hover chosen from pIdx (step 12), pressed
    // from bgIdx (step 9).
    let background = container(
        neutral(
            bg_comp,
            accent,
            on_bg_comp,
            hover_overlay,
            bg_pressed_overlay,
            hc,
            body,
        ),
        bg,
        text(bg_idx, &bg_ramp, tint, actual_alpha),
        small(bg_idx, &neutral_ramp, controls[6]),
        hc,
    );
    // 17. Secondary container; its texts use bgRamp.
    let secondary_default = surface(bg_idx, 10, &bg_ramp, dark, controls[2]);
    let s_base = seeds.secondary_container_bg.unwrap_or(secondary_default);
    let s_ramp = ramp::<100>(s_base);
    let s_idx = idx(s_base);
    let s_comp = surface(s_idx, 3, &s_ramp, dark, controls[4]);
    let s_hover = with_alpha(chosen(s_idx), 0.1);
    let s_pressed = with_alpha(chosen(s_idx), 0.2);
    let secondary = container(
        neutral(
            s_comp,
            accent,
            text(idx(s_comp), &bg_ramp, tint, actual_alpha),
            s_hover,
            s_pressed,
            hc,
            body,
        ),
        s_base,
        text(s_idx, &bg_ramp, tint, actual_alpha),
        small(s_idx, &neutral_ramp, controls[6]),
        hc,
    );
    // 18. Accent.
    let accent_component = coloured(accent, controls[0], accent, button_hover, button_pressed);
    // 19. Accent button: its text from the 100-ramp of controls[5].
    let on_accent_ramp = ramp::<100>(controls[5]);
    let accent_button = coloured_button(
        accent,
        controls[1],
        text(idx(accent), &on_accent_ramp, tint, 1.0),
        accent,
        button_hover,
        button_pressed,
    );
    // 20. Button.
    let button = neutral(
        button_bg,
        accent,
        on_bg_comp,
        button_hover,
        button_pressed,
        hc,
        body,
    );
    // 21, 25, 27. The coloured families.
    let family = |seed| coloured(seed, controls[0], accent, button_hover, button_pressed);
    let family_button = |seed, overlay| {
        coloured_button(
            seed,
            overlay,
            controls[0],
            accent,
            button_hover,
            button_pressed,
        )
    };
    // 22. Icon button.
    let icon_button = neutral(
        TRANSPARENT,
        accent,
        body,
        button_hover,
        button_pressed,
        hc,
        body,
    );
    // 23. Link button; its on_disabled is Over(its on at 0.5, its base).
    let link_text = accent_text.unwrap_or(accent);
    let mut link_button = neutral(
        TRANSPARENT,
        accent,
        link_text,
        TRANSPARENT,
        TRANSPARENT,
        hc,
        body,
    );
    link_button.on_disabled = over(with_alpha(link_button.on, 0.5), link_button.base);
    // 24. List button.
    let list_button = neutral(
        TRANSPARENT,
        accent,
        on_bg_comp,
        TRANSPARENT,
        button_pressed,
        hc,
        body,
    );
    // 26. Text button.
    let text_button = neutral(
        TRANSPARENT,
        accent,
        link_text,
        button_hover,
        button_pressed,
        hc,
        body,
    );
    // 28. Transparent background: hover and pressed chosen from sIdx.
    let transparent_background = container(
        neutral(tbg_comp, accent, on_tbg_comp, s_hover, s_pressed, hc, body),
        tbg,
        text(bg_idx, &bg_ramp, tint, actual_alpha),
        small(bg_idx, &neutral_ramp, controls[6]),
        hc,
    );
    // 29. Transparent secondary; its texts use its own ramp.
    let ts_alpha = (container_alpha + if dark { 0.6 } else { 0.5 }).min(1.0);
    let ts_base = with_alpha(s_base, ts_alpha);
    let ts_ramp = ramp::<100>(ts_base);
    let ts_idx = idx(ts_base);
    let ts_comp = surface(ts_idx, 3, &ts_ramp, dark, controls[4]);
    let transparent_secondary = container(
        neutral(
            ts_comp,
            accent,
            text(idx(ts_comp), &ts_ramp, tint, actual_alpha),
            s_hover,
            s_pressed,
            hc,
            body,
        ),
        ts_base,
        text(ts_idx, &ts_ramp, tint, actual_alpha),
        small(ts_idx, &neutral_ramp, controls[6]),
        hc,
    );
    // 30. Copied through.
    DerivedTheme {
        palette: *palette,
        is_dark: dark,
        is_high_contrast: hc,
        background,
        primary,
        secondary,
        transparent_background,
        transparent_primary,
        transparent_secondary,
        accent: accent_component,
        accent_button,
        button,
        destructive: family(destructive),
        destructive_button: family_button(destructive, controls[1]),
        icon_button,
        link_button,
        list_button,
        success: family(success),
        success_button: family_button(success, controls[1]),
        text_button,
        warning: family(warning),
        warning_button: family_button(warning, controls[10]),
        accent_text,
        shade,
        control_tint: seeds.neutral_tint,
        text_tint: seeds.text_tint,
        window_hint: seeds.window_hint,
        frosted: *frosted,
    }
}
