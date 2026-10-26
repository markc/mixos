// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The shell's look, prepared on the Bus worker from an accepted settings
//! snapshot and activated as one value after the session fence: the MixOS
//! theme the desktop profile describes (`toolkit::theme::Theme`, the one the
//! apps draw with), its text scale, the panel preferences, and the
//! server-side decoration themes `decor` draws window frames with.
use decor::{
    ChromeTheme, Palette,
    layout::{ChromeStyle, DecoFontFamily, DecoFontWeight},
};
use design::{
    CaptionSide, Contrast, DesignContext, Mode, RequestedDecorations, Scheme, Style,
    TypographyGeneric, TypographyRole,
};
use settings::{Diagnostic, Snapshot};
use std::sync::Arc;
use toolkit::theme::Theme;

/// The effective context the shell draws in.
pub(crate) const CONTEXT: &str = "desktop";

#[derive(Clone, Debug)]
pub struct Look {
    pub theme: Arc<Theme>,
    /// The profile's text scale (`ui.text_scale`), applied to every text style.
    pub text_scale: f32,
    pub(crate) preferences: crate::preferences::Preferences,
    chrome: Vec<ChromeTheme>,
}

fn unsupported(path: &str, message: &str) -> Diagnostic {
    Diagnostic::new("unsupported_presentation", path, message)
}

/// The theme an accepted snapshot describes: its own design source when the
/// profile carries one, otherwise the embedded design, either way at the
/// effective scheme, style, mode and contrast settingsd accepted, framed as
/// the profile asks.
fn theme_of(snapshot: &Snapshot) -> Result<Theme, Diagnostic> {
    let effective = snapshot.effective.get(CONTEXT).ok_or_else(|| {
        Diagnostic::new(
            "missing_context",
            "effective",
            "Snapshot has no desktop context",
        )
    })?;
    let decorations = RequestedDecorations::from_name(&effective.decorations)
        .ok_or_else(|| unsupported("appearance.decorations", "Unknown decorations"))?;
    let captions = CaptionSide::from_name(&effective.caption_side)
        .ok_or_else(|| unsupported("appearance.caption_side", "Unknown caption side"))?;
    let style = match &effective.style {
        None => None,
        Some(name) => Some(
            Style::from_name(name)
                .ok_or_else(|| unsupported("appearance.style", "Unknown style"))?,
        ),
    };
    let context = DesignContext {
        scheme: Scheme::from_name(&effective.scheme)
            .ok_or_else(|| unsupported("appearance.scheme", "Unknown scheme"))?,
        mode: Mode::from_name(&effective.mode)
            .ok_or_else(|| unsupported("appearance.mode", "Unknown mode"))?,
        contrast: Contrast::from_name(&effective.contrast)
            .ok_or_else(|| unsupported("appearance.contrast", "Unknown contrast"))?,
        style,
        ..DesignContext::default()
    };
    let theme = match &snapshot.desktop.appearance.source {
        Some(source) => {
            Theme::from_source_in("settings:desktop", source, context).map_err(|error| {
                unsupported(
                    "appearance.source",
                    &format!("Design does not compile: {error:?}"),
                )
            })?
        }
        None => Theme::for_context(context),
    };
    Ok(theme.framed(decorations, captions))
}

/// Build the whole look from an accepted snapshot (on the worker).
pub(crate) fn build(snapshot: &Snapshot) -> Result<Look, Diagnostic> {
    let preferences = crate::preferences::Preferences::prepare(&snapshot.desktop.shell)?;
    let theme = theme_of(snapshot)?;
    let text_scale = snapshot
        .effective
        .get(CONTEXT)
        .map_or(1.0, |effective| effective.ui.text_scale as f32)
        .clamp(0.5, 3.0);
    let title = design::active_typography(Some(theme.typography()), TypographyRole::UiDisplay);
    let family = match title.generic {
        TypographyGeneric::Monospace => DecoFontFamily::Monospace,
        _ if title.family.is_empty() || title.family == "Inter" => DecoFontFamily::SystemUi,
        _ => DecoFontFamily::Named(title.family.clone()),
    };
    let chrome = ChromeStyle::ALL
        .into_iter()
        .map(|style| {
            ChromeTheme::from_read(
                style,
                theme.dictionary(),
                family.clone(),
                DecoFontWeight(title.weight),
                title.font_size as f32 * text_scale,
            )
        })
        .collect();
    Ok(Look {
        theme: Arc::new(theme),
        text_scale,
        preferences,
        chrome,
    })
}

impl Look {
    pub fn chrome(&self, style: ChromeStyle) -> ChromeTheme {
        self.chrome
            .iter()
            .find(|theme| theme.deco.style == style)
            .expect("every chrome style was prepared")
            .clone()
    }

    /// The scene palette: the theme's, with an edge page on the secondary
    /// pair (Quoin's PANEL token); a dialog keeps the base pair.
    pub fn palette(&self, dialog: bool) -> Palette {
        page(&self.theme, dialog)
    }
}

/// [`Look::palette`] for any theme.
pub(crate) fn page(theme: &Theme, dialog: bool) -> Palette {
    let mut palette = Palette::from_dictionary(theme.dictionary());
    if !dialog {
        palette.base = palette.secondary;
    }
    palette
}

/// Install `theme` on a scene's egui context at `text_scale`, with an edge
/// page's surface and ink on the secondary pair.
pub(crate) fn install(ctx: &egui::Context, theme: &Theme, text_scale: f32, dialog: bool) {
    toolkit::install(ctx, theme);
    let palette = page(theme, dialog);
    let fill = crate::view::colour(palette.base.surface);
    let ink = crate::view::colour(palette.base.foreground);
    ctx.global_style_mut(|style| {
        if (text_scale - 1.0).abs() > f32::EPSILON {
            for font in style.text_styles.values_mut() {
                font.size *= text_scale;
            }
        }
        style.visuals.panel_fill = fill;
        style.visuals.window_fill = fill;
        style.visuals.override_text_color = Some(ink);
    });
}

#[cfg(test)]
pub(crate) fn fixture(mode: &str, scale: f64) -> Arc<Look> {
    let mut desktop = settings::Desktop::default();
    desktop.appearance.mode = mode.into();
    desktop.ui.text_scale = scale;
    let effective = settings::resolve(&desktop).unwrap();
    let snapshot = Snapshot {
        schema: settings::SCHEMA,
        binding: settings::Binding {
            instance: "desktop".into(),
            profile: "test".into(),
        },
        incarnation: "test".into(),
        revision: settings::Revision(1),
        design_revision: settings::Revision(1),
        source_digest: settings::source_digest(settings::EMBEDDED_DEFAULT_SOURCE),
        desktop,
        effective,
    };
    Arc::new(build(&snapshot).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_look_follows_mode_and_text_scale() {
        let dark = fixture("dark", 1.0);
        let light = fixture("light", 1.5);
        assert_eq!(dark.theme.mode(), Mode::Dark);
        assert_eq!(light.theme.mode(), Mode::Light);
        assert_eq!(light.text_scale, 1.5);
        assert_ne!(
            dark.palette(false).base,
            dark.palette(true).base,
            "an edge page is on the secondary pair"
        );
        let title = light.chrome(ChromeStyle::ALL[0]);
        let unscaled = dark.chrome(ChromeStyle::ALL[0]);
        assert!(title.deco.metrics.title_size_px > unscaled.deco.metrics.title_size_px);
    }

    /// A profile's own design source is drawn at the selection settingsd
    /// accepted, not at the defaults the source document names.
    #[test]
    fn a_custom_source_follows_the_accepted_selection() {
        let mut desktop = settings::Desktop::default();
        desktop.appearance.mode = "light".into();
        desktop.appearance.contrast = "high".into();
        desktop.appearance.source = Some(settings::EMBEDDED_DEFAULT_SOURCE.into());
        let effective = settings::resolve(&desktop).unwrap();
        let selected = effective.get(CONTEXT).unwrap().clone();
        let snapshot = Snapshot {
            schema: settings::SCHEMA,
            binding: settings::Binding {
                instance: "desktop".into(),
                profile: "test".into(),
            },
            incarnation: "test".into(),
            revision: settings::Revision(1),
            design_revision: settings::Revision(1),
            source_digest: settings::source_digest(settings::EMBEDDED_DEFAULT_SOURCE),
            desktop,
            effective,
        };
        let look = build(&snapshot).unwrap();
        assert_eq!(look.theme.mode(), Mode::from_name(&selected.mode).unwrap());
        assert_eq!(
            look.theme.contrast(),
            Contrast::from_name(&selected.contrast).unwrap()
        );
        assert_eq!(look.theme.mode(), Mode::Light);
        assert_eq!(look.theme.contrast(), Contrast::from_name("high").unwrap());
    }
}
