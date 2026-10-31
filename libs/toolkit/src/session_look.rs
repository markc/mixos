// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The session's look as settingsd holds it, turned into a [`Theme`]. Pure:
//! no Bus and no I/O. The app reads the desktop profile (for example with
//! `settings::follow::Follower`) and hands its appearance here.
//!
//! The appearance names the embedded design's scheme, style, mode and
//! contrast, and the framing (decorations, caption side). When it names a
//! design package of its own (`source`), or a name this build does not know,
//! the app's session design stands. `theme_or` still applies the independent
//! framing and font choices, so a newer or custom desktop never breaks an older app.
//!
//! Key space v2's follower view, [`settings::view::Look`], is drawn by
//! [`look`] and [`look_or`] in R5's order: the axes (the app's overrides,
//! the resolved mode), the compile with the accent and the derived scheme's
//! tints, `design::geometry` (roundness, density), the font choices, then
//! the session's `tune.*`, which beat the geometry they override. The
//! format-4 entry points stay as they are until the apps move onto `Look`
//! (S8a).
use crate::theme::{Overlay, Theme};
use design::{
    CaptionSide, Contrast, DesignContext, Mode, RequestedDecorations as Decorations, Scheme, Style,
};
use settings::model::Appearance;
use settings::view::{Accent, Framing, Look};

/// The theme `appearance` describes, or `None` when this build cannot draw
/// it (a design package of its own, or an unknown name): keep the session
/// theme then.
pub fn theme(appearance: &Appearance) -> Option<Theme> {
    if appearance.source.is_some() {
        return None;
    }
    let style = match &appearance.style {
        None => None,
        Some(name) => Some(Style::from_name(name)?),
    };
    let context = DesignContext {
        scheme: Scheme::from_name(&appearance.scheme)?,
        mode: Mode::from_name(&appearance.mode)?,
        contrast: Contrast::from_name(&appearance.contrast)?,
        style,
        ..DesignContext::default()
    };
    let decorations = Decorations::from_name(&appearance.decorations)?;
    let captions = CaptionSide::resolve(&appearance.caption_side);
    // The icon and type choices ride on the theme, so every app that installs
    // this look draws with them; a choice left unset keeps the design's value.
    Theme::for_context(context)
        .framed(decorations, captions)
        .with_menus(
            design::MenuStyle::resolve(&appearance.menu_style),
            design::MenuSide::resolve(&appearance.menu_side),
        )
        .with_band(band(appearance))
        .with_overlay(Overlay::from_appearance(appearance))
        .ok()
}

/// [`theme`], or the session's design with the profile's framing when the
/// appearance cannot be drawn.
pub fn theme_or(appearance: &Appearance, session: &Theme) -> Theme {
    follow(appearance, false, session)
}

/// Follow a summary's appearance. Summaries omit the source itself, so the
/// separate `custom_source` flag keeps the file's design while applying the
/// profile's framing, menus, band and font choices.
pub fn follow(appearance: &Appearance, custom_source: bool, session: &Theme) -> Theme {
    let selected = if custom_source {
        None
    } else {
        theme(appearance)
    };
    selected.unwrap_or_else(|| {
        // Framing is independent of the design package: an unsupported
        // source must still follow the session's requested decorations.
        let selected = Overlay::from_appearance(appearance);
        let mut overlay = session.overlay().clone();
        overlay.weight = selected.weight;
        overlay.size = selected.size;
        session
            .clone()
            .framed(
                Decorations::from_name(&appearance.decorations)
                    .unwrap_or(session.requested_decorations()),
                CaptionSide::resolve(&appearance.caption_side),
            )
            .with_menus(
                design::MenuStyle::resolve(&appearance.menu_style),
                design::MenuSide::resolve(&appearance.menu_side),
            )
            .with_band(band(appearance))
            .with_overlay(overlay)
            .expect("the session's style overlay is already valid")
    })
}

pub fn band(appearance: &Appearance) -> design::title_band::Settings {
    design::title_band::Settings::from_names(
        &appearance.title_align,
        &appearance.actions_align,
        &appearance.actions_place,
        &appearance.window_style,
    )
}

/// The title band `framing` names.
pub fn band_of(framing: &Framing) -> design::title_band::Settings {
    design::title_band::Settings::from_names(
        &framing.title_align,
        &framing.actions_align,
        &framing.actions_place,
        &framing.style,
    )
}

/// The theme `look` describes for `app` (`None` is the desktop), or `None`
/// when this build cannot draw it (a design source of its own, or a name
/// it does not know): keep the session theme then ([`look_or`]).
pub fn look(look: &Look, app: Option<&str>) -> Option<Theme> {
    let appearance = &look.appearance;
    if appearance.source.is_some() {
        return None;
    }
    // R5 step 1: the axes, an app's overrides over the desktop's.
    let overlay = app.and_then(|id| look.apps.0.get(id));
    let scheme = overlay
        .and_then(|o| o.scheme.as_deref())
        .unwrap_or(&appearance.scheme);
    let scheme = Scheme::from_name(scheme)?;
    let mode = match overlay.and_then(|o| o.mode.as_deref()) {
        Some(name) => Mode::from_name(name)?,
        None => Mode::from_name(look.mode().0.name())?,
    };
    // A requested high contrast cannot be cancelled by an app.
    let contrast = match overlay.and_then(|o| o.contrast.as_deref()) {
        Some(name) if appearance.contrast != "high" => name,
        _ => &appearance.contrast,
    };
    let style = match &appearance.style {
        None => None,
        Some(name) => Some(Style::from_name(name)?),
    };
    // Step 2: the compile, with the accent and, under the derived scheme
    // only, the tints (an authored scheme has no slot for them, A3).
    let accent = match &appearance.accent {
        Accent::Auto => design::AccentChoice::Auto,
        Accent::Swatch(text) | Accent::Colour(text) => design::AccentChoice::from_name(text)?,
    };
    let tint = |value: &Option<String>| match value {
        None => Some(None),
        Some(hex) => design::Colour8::from_hex(hex).map(Some),
    };
    let tints = if scheme.is_derived() {
        design::Tints {
            window_bg: tint(&appearance.window_bg)?,
            container_bg: tint(&appearance.container_bg)?,
            text_tint: tint(&appearance.text_tint)?,
            control_tint: tint(&appearance.control_tint)?,
        }
    } else {
        design::Tints::default()
    };
    let context = DesignContext {
        scheme,
        mode,
        contrast: Contrast::from_name(contrast)?,
        style,
        accent,
        tints,
        ..DesignContext::default()
    };
    let framing = &look.framing;
    let decorations = Decorations::from_name(&framing.decorations)?;
    let theme = Theme::for_context(context)
        .framed(decorations, CaptionSide::resolve(&framing.caption_side))
        .with_menus(
            design::MenuStyle::resolve(&framing.menu_style),
            design::MenuSide::resolve(&framing.menu_side),
        )
        .with_band(band_of(framing))
        // Step 7, and the step's multiplier for the legacy lengths (step
        // 2a's density half, until the toolkit applies the step itself).
        .with_ui(settings::model::CommonUi {
            density: appearance.density.multiplier(),
            text_scale: overlay
                .and_then(|o| o.text_scale)
                .unwrap_or(look.text_scale),
            reduced_motion: look.reduced_motion,
        });
    let overlay = look_overlay(look, &theme)?;
    theme.with_overlay(overlay).ok()
}

/// [`look`], or the session's design with the look's framing, menus, band
/// and font choices when the look cannot be drawn.
pub fn look_or(look: &Look, app: Option<&str>, session: &Theme) -> Theme {
    self::look(look, app).unwrap_or_else(|| framed_session(look, session))
}

/// The session's design with the look's framing, menus, band and font
/// choices: what [`look_or`] draws when the look cannot be drawn, and what a
/// profile that names a design source of its own keeps.
pub fn framed_session(look: &Look, session: &Theme) -> Theme {
    let selected = fonts(look);
    let mut overlay = session.overlay().clone();
    overlay.weight = selected.weight;
    overlay.size = selected.size;
    let framing = &look.framing;
    session
        .clone()
        .framed(
            Decorations::from_name(&framing.decorations).unwrap_or(session.requested_decorations()),
            CaptionSide::resolve(&framing.caption_side),
        )
        .with_menus(
            design::MenuStyle::resolve(&framing.menu_style),
            design::MenuSide::resolve(&framing.menu_side),
        )
        .with_band(band_of(framing))
        .with_overlay(overlay)
        .expect("the session's style overlay is already valid")
}

fn fonts(look: &Look) -> Overlay {
    let a = &look.appearance;
    Overlay::from_choices(
        a.icon_stroke,
        a.font_family.as_deref(),
        a.font_weight,
        a.font_size,
        a.mono_family.as_deref(),
    )
}

/// The overlay `look` lays over `theme`'s compiled style: the geometry's
/// radius and spacing tokens (R5 step 2a; the legacy lengths scale by the
/// density in `ui_scale`, exactly as `design::geometry::legacy_lengths`
/// does), the font choices (step 3), then the session's tunes (step 4),
/// which beat the geometry. `None` when a tune has no token form.
fn look_overlay(look: &Look, theme: &Theme) -> Option<Overlay> {
    let geometry = look.geometry();
    let raw = theme.raw_style();
    let shaped = design::geometry::spacing(
        design::geometry::round(raw, geometry.roundness),
        geometry.density,
    );
    let before = raw.values();
    let mut overlay = fonts(look);
    overlay.style = shaped
        .values()
        .into_iter()
        .filter(|(name, value)| before.get(name) != Some(value))
        .collect();
    for (name, value) in &look.tune.0 {
        if let Some(token) = name.strip_prefix("style.") {
            let value = match value {
                serde_json::Value::Bool(flag) => design::StyleValueSource::Flag(*flag),
                serde_json::Value::Number(n) => design::StyleValueSource::Number(n.as_f64()?),
                serde_json::Value::String(s) => design::StyleValueSource::Name(s.clone()),
                _ => return None,
            };
            overlay.style.insert(token.to_owned(), value);
        } else if let Some(role) = name.strip_prefix("chrome.") {
            let colour = design::parse_legacy_v0_hex_colour(value.as_str()?).ok()?;
            overlay
                .chrome
                .insert(role.to_owned(), design::family::style::StyleColour(colour));
        }
    }
    Some(overlay)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn appearance(scheme: &str) -> Appearance {
        Appearance {
            scheme: scheme.into(),
            ..Appearance::default()
        }
    }

    #[test]
    fn every_axis_reaches_the_theme() {
        let a = Appearance {
            scheme: "forest".into(),
            style: Some("pro".into()),
            mode: "light".into(),
            contrast: "high".into(),
            decorations: "ssd".into(),
            caption_side: "left".into(),
            window_style: "win11".into(),
            menu_style: "titlebar".into(),
            menu_side: "right".into(),
            title_align: "auto".into(),
            actions_align: "right".into(),
            actions_place: "row".into(),
            icon_stroke: None,
            font_family: None,
            font_weight: None,
            font_size: None,
            mono_family: None,
            source: None,
        };
        let theme = theme(&a).expect("drawable");
        assert_eq!(theme.scheme(), Scheme::Forest);
        assert_eq!(theme.style_axis(), Some(Style::Pro));
        assert_eq!(theme.mode(), Mode::Light);
        assert_eq!(theme.contrast(), Contrast::High);
        assert_eq!(theme.decorations(), design::Decorations::Server);
        assert_eq!(theme.captions(), CaptionSide::Left);
        assert_eq!(theme.menu_style(), design::MenuStyle::Titlebar);
        assert_eq!(theme.menu_side(), design::MenuSide::Right);
    }

    #[test]
    fn unsupported_designs_still_follow_the_session_framing() {
        let appearance = Appearance {
            source: Some("{}".into()),
            decorations: "ssd".into(),
            ..Appearance::default()
        };
        let session = Theme::embedded();
        let framed = theme_or(&appearance, &session);
        assert_eq!(framed.dictionary(), session.dictionary());
        assert_eq!(framed.decorations(), design::Decorations::Server);
    }

    #[test]
    fn a_font_size_reaches_the_overlay_only_in_range() {
        use crate::theme::Overlay;
        let sized = |size: f64| Appearance {
            font_size: Some(size),
            ..Appearance::default()
        };
        assert_eq!(Overlay::from_appearance(&sized(14.0)).size, Some(14.0));
        assert_eq!(Overlay::from_appearance(&sized(30.0)).size, None);
        assert_eq!(Overlay::from_appearance(&Appearance::default()).size, None);
    }

    #[test]
    fn the_default_appearance_is_the_embedded_default() {
        let theme = theme(&Appearance::default()).expect("drawable");
        let embedded = Theme::embedded();
        assert_eq!(
            (theme.scheme(), theme.mode(), theme.style_axis()),
            (embedded.scheme(), embedded.mode(), embedded.style_axis())
        );
    }

    #[test]
    fn unknown_names_and_own_packages_keep_the_session_theme() {
        let session = Theme::for_context(DesignContext {
            scheme: Scheme::Ocean,
            ..DesignContext::default()
        });
        for a in [
            appearance("no-such-scheme"),
            Appearance {
                style: Some("no-such-style".into()),
                ..appearance("forest")
            },
            Appearance {
                source: Some("{}".into()),
                ..appearance("forest")
            },
        ] {
            assert!(theme(&a).is_none(), "{a:?}");
            assert_eq!(theme_or(&a, &session).scheme(), Scheme::Ocean);
        }
    }

    #[test]
    fn font_choices_survive_an_unsupported_design() {
        let session = Theme::embedded()
            .with_overlay(Overlay {
                reduced_motion: true,
                ..Default::default()
            })
            .unwrap();
        for mut appearance in [
            appearance("no-such-scheme"),
            Appearance {
                source: Some("{}".into()),
                ..Appearance::default()
            },
        ] {
            appearance.font_weight = Some(700);
            appearance.font_size = Some(18.0);
            let chosen = theme_or(&appearance, &session);
            assert_eq!(chosen.overlay().weight, Some(700));
            assert_eq!(chosen.overlay().size, Some(18.0));
            assert!(chosen.overlay().reduced_motion);
        }
    }

    #[test]
    fn summary_source_flag_preserves_design_and_file_overlay() {
        let session = Theme::for_context(DesignContext {
            scheme: Scheme::Ocean,
            mode: Mode::Light,
            ..Default::default()
        })
        .with_overlay(Overlay {
            reduced_motion: true,
            stroke: Some(3.0),
            ..Default::default()
        })
        .unwrap();
        for (weight, size) in [(None, None), (Some(700), Some(18.0))] {
            let appearance = Appearance {
                scheme: "forest".into(),
                font_weight: weight,
                font_size: size,
                ..Default::default()
            };
            for custom_source in [false, true] {
                let chosen = follow(&appearance, custom_source, &session);
                let expected = if custom_source {
                    session.clone()
                } else {
                    theme(&appearance).unwrap()
                };
                assert_eq!(chosen.dictionary(), expected.dictionary());
                assert_eq!(chosen.context(), expected.context());
                assert_eq!(chosen.overlay().weight, weight);
                assert_eq!(chosen.overlay().size, size.map(|v| v as f32));
                assert_eq!(chosen.overlay().stroke, expected.overlay().stroke);
                assert_eq!(
                    chosen.overlay().reduced_motion,
                    expected.overlay().reduced_motion
                );
            }
        }
    }

    #[test]
    fn unset_choices_leave_the_design_alone() {
        let plain = theme(&appearance("forest")).unwrap();
        assert_eq!(plain.overlay(), &Overlay::default());
        assert_eq!(
            crate::icons::stroke_width(&plain),
            crate::icons::stroke_width(&Theme::for_context(DesignContext {
                scheme: Scheme::Forest,
                ..DesignContext::default()
            })),
        );
    }

    #[test]
    fn a_chosen_stroke_and_weight_reach_the_theme() {
        let a = Appearance {
            icon_stroke: Some(2.5),
            font_weight: Some(500),
            ..appearance("forest")
        };
        let theme = theme(&a).unwrap();
        assert_eq!(crate::icons::stroke_width(&theme), 2.5);
        assert_eq!(theme.overlay().weight, Some(500));
    }

    fn look_of(changes: &[(&str, serde_json::Value)]) -> Look {
        use settings::view::View;
        let changes = changes
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        let values =
            settings::registry::apply(&settings::registry::Values::new(), &changes, &[]).unwrap();
        Look::of(&values)
    }

    #[test]
    fn the_default_look_is_the_derived_mixos_and_round() {
        let theme = look(&look_of(&[]), None).expect("drawable");
        assert_eq!(theme.scheme(), Scheme::Mixos);
        assert_eq!(theme.mode(), Mode::Dark);
        let style = theme.style();
        let [_, xs, s, m, _, xl] = design::geometry::Roundness::Round.radii();
        assert_eq!(
            (
                style.radius_sm,
                style.radius,
                style.radius_lg,
                style.radius_pill
            ),
            (xs, s, m, xl)
        );
        assert_eq!(theme.ui().density, 1.0);
    }

    /// Under an authored scheme at the defaults (accent `auto`, `round`,
    /// `comfortable`), the look draws today's design and today's style but
    /// for the roundness preset's radii (F2: "the same look except corner
    /// radii") and the new spacing tokens' comfortable row.
    #[test]
    fn an_authored_scheme_at_the_defaults_is_today_s_theme_but_for_radii() {
        for scheme in Scheme::AUTHORED {
            let today = theme(&appearance(scheme.name())).unwrap();
            let next = look(
                &look_of(&[("appearance.scheme", scheme.name().into())]),
                None,
            )
            .expect("drawable");
            assert_eq!(next.dictionary(), today.dictionary(), "{scheme:?}");
            assert_eq!(next.accent(), today.accent(), "{scheme:?}");
            assert_eq!(
                next.style(),
                design::geometry::spacing(
                    design::geometry::round(today.style(), design::geometry::Roundness::Round),
                    design::geometry::Density::Standard
                ),
                "{scheme:?}"
            );
        }
    }

    #[test]
    fn the_accent_and_the_mode_reach_the_theme() {
        for mode in [Mode::Dark, Mode::Light] {
            let theme = look(
                &look_of(&[
                    ("appearance.scheme", "studio".into()),
                    ("appearance.accent", "green".into()),
                    ("appearance.mode", mode.name().into()),
                ]),
                None,
            )
            .unwrap();
            assert_eq!(theme.mode(), mode);
            assert_eq!(
                theme.accent().map(|a| a.to_hex()),
                Some(design::Swatch::Green.colour(mode).to_hex())
            );
        }
        let auto = look_of(&[
            ("appearance.mode", "auto".into()),
            ("state.appearance.daylight", true.into()),
        ]);
        assert_eq!(look(&auto, None).unwrap().mode(), Mode::Light);
    }

    #[test]
    fn a_tint_reaches_the_derived_scheme_only() {
        let tinted = |scheme: &str| {
            look(
                &look_of(&[
                    ("appearance.scheme", scheme.into()),
                    ("appearance.window_bg", "#203040".into()),
                ]),
                None,
            )
            .unwrap()
        };
        let plain =
            |scheme: &str| look(&look_of(&[("appearance.scheme", scheme.into())]), None).unwrap();
        assert_ne!(tinted("mixos").dictionary(), plain("mixos").dictionary());
        assert_eq!(tinted("studio").dictionary(), plain("studio").dictionary());
    }

    /// R5: a tune beats the roundness it overrides; density scales the
    /// legacy lengths through the step's multiplier.
    #[test]
    fn a_tune_beats_the_geometry() {
        let theme = look(
            &look_of(&[
                ("appearance.roundness", "square".into()),
                ("appearance.density", "compact".into()),
                ("tune.style.radius", 5.into()),
            ]),
            None,
        )
        .unwrap();
        let style = theme.style();
        assert_eq!((style.radius, style.radius_sm), (5, 0));
        assert_eq!(theme.ui().density, 0.85);
        let plain = look(&look_of(&[]), None).unwrap();
        assert_eq!(
            style.field_height,
            plain.raw_style().field_height * 0.85,
            "the legacy lengths scale once"
        );
    }

    #[test]
    fn an_app_override_applies_to_that_app_only() {
        let l = look_of(&[
            ("apps.term.mode", "light".into()),
            ("apps.term.text_scale", serde_json::json!(1.5)),
            ("appearance.contrast", "high".into()),
            ("apps.term.contrast", "normal".into()),
        ]);
        let term = look(&l, Some("term")).unwrap();
        assert_eq!(term.mode(), Mode::Light);
        assert_eq!(term.ui().text_scale, 1.5);
        assert_eq!(term.contrast(), Contrast::High, "high cannot be cancelled");
        let desktop = look(&l, None).unwrap();
        assert_eq!(desktop.mode(), Mode::Dark);
        assert_eq!(desktop.ui().text_scale, 1.0);
    }

    #[test]
    fn a_look_with_its_own_source_keeps_the_session_design() {
        let source = design::EMBEDDED_DEFAULT_SOURCE.to_owned();
        let l = look_of(&[
            ("appearance.source", source.into()),
            ("window.decorations", "ssd".into()),
            ("appearance.font_weight", 700.into()),
        ]);
        assert!(look(&l, None).is_none());
        let session = Theme::embedded();
        let chosen = look_or(&l, None, &session);
        assert_eq!(chosen.dictionary(), session.dictionary());
        assert_eq!(chosen.decorations(), design::Decorations::Server);
        assert_eq!(chosen.overlay().weight, Some(700));
    }

    #[test]
    fn out_of_range_choices_degrade_to_the_design() {
        let bad = Appearance {
            icon_stroke: Some(2.1),
            font_weight: Some(350),
            ..appearance("forest")
        };
        let overlay = Overlay::from_appearance(&bad);
        assert_eq!(overlay, Overlay::default());
        assert!(theme(&bad).is_some(), "a bad choice never breaks the theme");
    }
}
