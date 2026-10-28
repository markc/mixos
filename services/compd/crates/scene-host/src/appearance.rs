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
use toolkit::theme::{Overlay, Theme};

/// The effective context the shell draws in.
pub(crate) const CONTEXT: &str = "desktop";

#[derive(Clone, Debug)]
pub struct Look {
    pub theme: Arc<Theme>,
    /// The profile's text scale (`ui.text_scale`), applied to every text style.
    pub text_scale: f32,
    pub window_style: ChromeStyle,
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
    let captions = CaptionSide::resolve(&effective.caption_side);
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
    let theme = theme
        .with_overlay(Overlay::from_appearance(&snapshot.desktop.appearance))
        .map_err(|error| unsupported("appearance", &format!("Invalid overlay: {error:?}")))?;
    Ok(theme
        .framed(decorations, captions)
        .with_menus(
            design::MenuStyle::resolve(&effective.menu_style),
            design::MenuSide::resolve(&effective.menu_side),
        )
        .with_band(design::title_band::Settings::from_names(
            &effective.title_align,
            &effective.actions_align,
            &effective.actions_place,
            &effective.window_style,
        )))
}

/// Build the whole look from an accepted snapshot (on the worker).
pub(crate) fn build(snapshot: &Snapshot) -> Result<Look, Diagnostic> {
    let preferences = crate::preferences::Preferences::prepare(&snapshot.desktop.shell)?;
    let theme = theme_of(snapshot)?;
    let window_style = snapshot
        .effective
        .get(CONTEXT)
        .map_or_else(ChromeStyle::default, |effective| {
            ChromeStyle::resolve(&effective.window_style)
        });
    let text_scale = snapshot
        .effective
        .get(CONTEXT)
        .map_or(1.0, |effective| effective.ui.text_scale as f32)
        .clamp(0.5, 3.0);
    let title = design::active_typography(Some(theme.typography()), TypographyRole::UiDisplay);
    let title_weight = if theme.overlay().weight.is_some() {
        toolkit::fonts::weights(&theme).1
    } else {
        title.weight
    };
    let size_ratio = theme.interface_size_scale();
    let family = match title.generic {
        TypographyGeneric::Monospace => DecoFontFamily::Monospace,
        _ if title.family.is_empty() || title.family == "Inter" => DecoFontFamily::SystemUi,
        _ => DecoFontFamily::Named(title.family.clone()),
    };
    let chrome = ChromeStyle::ALL
        .into_iter()
        .map(|style| {
            let mut chrome = ChromeTheme::from_read(
                style,
                theme.dictionary(),
                family.clone(),
                DecoFontWeight(title_weight),
                title.font_size as f32 * size_ratio * text_scale,
            );
            chrome.menu_style = theme.menu_style();
            chrome.menu_side = theme.menu_side();
            chrome.band = theme.band();
            chrome.captions = theme.captions();
            chrome.action_style = theme.style();
            chrome.action_stroke = snapshot
                .effective
                .get(CONTEXT)
                .and_then(|e| e.icon_stroke)
                .map(|stroke| stroke as f32)
                .unwrap_or_else(|| toolkit::icons::stroke_width(&theme));
            chrome
        })
        .collect();
    Ok(Look {
        theme: Arc::new(theme),
        text_scale,
        window_style,
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
    let mut ui = theme.ui().clone();
    ui.text_scale *= f64::from(text_scale);
    let scaled = theme.clone().with_ui(ui);
    toolkit::install(ctx, &scaled);
    let palette = page(theme, dialog);
    let fill = crate::view::colour(palette.base.surface);
    let ink = crate::view::colour(palette.base.foreground);
    ctx.global_style_mut(|style| {
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
    Arc::new(build(&snapshot_of(desktop)).unwrap())
}

#[cfg(test)]
fn snapshot_of(desktop: settings::Desktop) -> Snapshot {
    let effective = settings::resolve(&desktop).unwrap();
    Snapshot {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_fonts_reach_scenes_titles_and_the_resource_change_plan() {
        let initial = snapshot_of(settings::Desktop::default());
        let plain = build(&initial).unwrap();
        assert_eq!(plain.theme.overlay(), &Overlay::default());
        let title =
            design::active_typography(Some(plain.theme.typography()), TypographyRole::UiDisplay);
        for style in ChromeStyle::ALL {
            let chrome = plain.chrome(style);
            assert_eq!(chrome.deco.metrics.title_font_weight.0, title.weight);
            assert_eq!(chrome.deco.metrics.title_size_px, title.font_size as f32);
        }
        let unscaled = egui::Context::default();
        install(&unscaled, &plain.theme, plain.text_scale, false);
        for weight in [300, 500, 700] {
            let mut desktop = initial.desktop.clone();
            desktop.appearance.font_weight = Some(weight);
            desktop.appearance.font_size = Some(18.0);
            desktop.ui.text_scale = 1.5;
            let next = snapshot_of(desktop);
            let changes =
                settings::domains::ChangePlan::between(Some(&initial), &next, CONTEXT, true);
            assert!(changes.resources && changes.text && changes.layout);
            let look = build(&next).unwrap();
            assert_eq!(look.theme.overlay().weight, Some(weight));
            assert_eq!(look.theme.overlay().size, Some(18.0));
            let ctx = egui::Context::default();
            install(&ctx, &look.theme, look.text_scale, false);
            let ui = design::active_typography(Some(plain.theme.typography()), TypographyRole::Ui);
            let ratio = 18.0 / ui.font_size as f32 * 1.5;
            let before = toolkit::chrome::Chrome::of(&unscaled).style;
            let after = toolkit::chrome::Chrome::of(&ctx).style;
            assert!((after.push_label_size - before.push_label_size * f64::from(ratio)).abs() < 0.001);
            assert!((after.panel_section_size - before.panel_section_size * f64::from(ratio)).abs() < 0.001);
            for text in [
                egui::TextStyle::Body,
                egui::TextStyle::Button,
                egui::TextStyle::Small,
                egui::TextStyle::Heading,
            ] {
                assert!(
                    (ctx.global_style().text_styles[&text].size
                        - unscaled.global_style().text_styles[&text].size * ratio)
                        .abs()
                        < 0.001
                );
            }
            let mut output = ctx.run_ui(Default::default(), |ui| {
                ui.fonts(|fonts| {
                    assert_eq!(
                        fonts.definitions().families[&egui::FontFamily::Proportional][0],
                        toolkit::fonts::inter_face(weight).0
                    );
                })
            });
            output.textures_delta.clear();
            for style in ChromeStyle::ALL {
                let chrome = look.chrome(style);
                assert_eq!(
                    chrome.deco.metrics.title_font_weight.0,
                    toolkit::fonts::weights(&look.theme).1
                );
                assert!(
                    (chrome.deco.metrics.title_size_px - title.font_size as f32 * ratio).abs()
                        < 0.001
                );
            }
        }
    }

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

    #[test]
    fn chrome_uses_the_effective_window_style_with_mac_for_unknown_names() {
        for name in ChromeStyle::NAMES.iter().copied().chain(["future-style"]) {
            let desktop = settings::Desktop::default();
            let mut effective = settings::resolve(&desktop).unwrap();
            effective.get_mut(CONTEXT).unwrap().window_style = name.into();
            let snapshot = Snapshot {
                schema: settings::SCHEMA,
                binding: settings::Binding {
                    instance: "fixture".into(),
                    profile: "default".into(),
                },
                incarnation: "test".into(),
                revision: settings::Revision(1),
                design_revision: settings::Revision(1),
                source_digest: settings::source_digest(settings::EMBEDDED_DEFAULT_SOURCE),
                desktop,
                effective,
            };
            let look = build(&snapshot).unwrap();
            assert_eq!(look.window_style, ChromeStyle::resolve(name));
            assert_eq!(
                look.chrome(look.window_style).deco.style,
                ChromeStyle::resolve(name)
            );
        }
    }

    #[test]
    fn menus_reach_every_chrome_style_from_the_effective_settings() {
        for style in design::MenuStyle::NAMES
            .iter()
            .copied()
            .chain(["future-style"])
        {
            for side in design::MenuSide::NAMES
                .iter()
                .copied()
                .chain(["future-side"])
            {
                let desktop = settings::Desktop::default();
                let mut effective = settings::resolve(&desktop).unwrap();
                effective.get_mut(CONTEXT).unwrap().menu_style = style.into();
                effective.get_mut(CONTEXT).unwrap().menu_side = side.into();
                let snapshot = Snapshot {
                    schema: settings::SCHEMA,
                    binding: settings::Binding {
                        instance: "fixture".into(),
                        profile: "default".into(),
                    },
                    incarnation: "test".into(),
                    revision: settings::Revision(1),
                    design_revision: settings::Revision(1),
                    source_digest: settings::source_digest(settings::EMBEDDED_DEFAULT_SOURCE),
                    desktop,
                    effective,
                };
                let look = build(&snapshot).unwrap();
                for window_style in ChromeStyle::ALL {
                    let chrome = look.chrome(window_style);
                    assert_eq!(chrome.menu_style, design::MenuStyle::resolve(style));
                    assert_eq!(chrome.menu_side, design::MenuSide::resolve(side));
                }
                assert_eq!(look.theme.menu_style(), design::MenuStyle::resolve(style));
                assert_eq!(look.theme.menu_side(), design::MenuSide::resolve(side));
            }
        }
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
