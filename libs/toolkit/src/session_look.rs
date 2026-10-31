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
        // The app axis, as resolution's `look_context` passes it. The
        // embedded design claims no app axis, so today this compiles as no
        // app; a design that claims it is a source of its own, which this
        // build does not draw (above).
        app: app.map(str::to_owned),
        accent,
        tints,
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
        .with_ui(look_ui(look, app));
    Some(lay(theme, fonts(look), look))
}

/// [`look`], or, when the look cannot be drawn, the session's design with
/// everything else the look says, built as [`look`] builds it: the framing,
/// menus and band, the ui (text scale, motion, density), the font choices,
/// the geometry and the tunes, laid over the session's own style.
pub fn look_or(look: &Look, app: Option<&str>, session: &Theme) -> Theme {
    self::look(look, app).unwrap_or_else(|| {
        let framing = &look.framing;
        let theme = session
            .clone()
            .framed(
                Decorations::from_name(&framing.decorations)
                    .unwrap_or(session.requested_decorations()),
                CaptionSide::resolve(&framing.caption_side),
            )
            .with_menus(
                design::MenuStyle::resolve(&framing.menu_style),
                design::MenuSide::resolve(&framing.menu_side),
            )
            .with_band(band_of(framing))
            .with_ui(look_ui(look, app));
        // The session's own style and chrome overrides stay beneath the
        // look's geometry and tunes.
        let mut base = fonts(look);
        base.style = session.overlay().style.clone();
        base.chrome = session.overlay().chrome.clone();
        base.reduced_motion = session.overlay().reduced_motion;
        lay(theme, base, look)
    })
}

/// R5 step 7 for `app`, and the density step's multiplier, which
/// `ui_scale` lays over the legacy lengths (step 2a's density half, until
/// the toolkit applies the step itself, S8a). It is the only place the
/// density scales them: [`lay`] sets no legacy length.
fn look_ui(look: &Look, app: Option<&str>) -> settings::model::CommonUi {
    let overlay = app.and_then(|id| look.apps.0.get(id));
    settings::model::CommonUi {
        density: look.appearance.density.multiplier(),
        text_scale: overlay
            .and_then(|o| o.text_scale)
            .unwrap_or(look.text_scale),
        reduced_motion: look.reduced_motion,
    }
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

/// The geometry's tokens over `raw` (R5 step 2a): the roundness preset's
/// radii and the density's new `spacing_*` and `header_padding_*` tokens,
/// as the tokens that differ from `raw`. No legacy length: `ui_scale`
/// scales those by the density's multiplier ([`look_ui`]), exactly as
/// `design::geometry::legacy_lengths` would.
fn geometry_tokens(
    look: &Look,
    raw: design::ResolvedStyle,
) -> std::collections::BTreeMap<String, design::StyleValueSource> {
    let geometry = look.geometry();
    let shaped = design::geometry::spacing(
        design::geometry::round(raw, geometry.roundness),
        geometry.density,
    );
    let before = raw.values();
    shaped
        .values()
        .into_iter()
        .filter(|(name, value)| before.get(name) != Some(value))
        .collect()
}

/// The session's tunes (R5 step 4) as overlay entries. A tune with no token
/// form is skipped, never the look: the store validated it, so this is a
/// second line.
fn tunes(look: &Look) -> Overlay {
    let mut overlay = Overlay::default();
    for (name, value) in &look.tune.0 {
        if let Some(token) = name.strip_prefix("style.") {
            let value = match value {
                serde_json::Value::Bool(flag) => design::StyleValueSource::Flag(*flag),
                serde_json::Value::Number(n) => match n.as_f64() {
                    Some(n) => design::StyleValueSource::Number(n),
                    None => continue,
                },
                serde_json::Value::String(s) => design::StyleValueSource::Name(s.clone()),
                _ => continue,
            };
            overlay.style.insert(token.to_owned(), value);
        } else if let Some(role) = name.strip_prefix("chrome.") {
            let Some(colour) = value
                .as_str()
                .and_then(|hex| design::parse_legacy_v0_hex_colour(hex).ok())
            else {
                continue;
            };
            overlay
                .chrome
                .insert(role.to_owned(), design::family::style::StyleColour(colour));
        }
    }
    overlay
}

/// `theme` with `base` (the font choices, and any overrides beneath the
/// look), then the geometry over `theme`'s style, then the tunes, which
/// beat the geometry. A layer the style refuses is dropped, the tunes
/// first, so a bad tune never costs the look its geometry or fonts.
fn lay(theme: Theme, base: Overlay, look: &Look) -> Theme {
    let mut geometry = base.clone();
    geometry
        .style
        .extend(geometry_tokens(look, theme.raw_style()));
    let mut tuned = geometry.clone();
    let tunes = tunes(look);
    tuned.style.extend(tunes.style);
    tuned.chrome.extend(tunes.chrome);
    for overlay in [tuned, geometry, base] {
        if let Ok(laid) = theme.clone().with_overlay(overlay) {
            return laid;
        }
    }
    theme
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

    /// The tokens the geometry may set: the six COSMIC radii, the legacy
    /// radii they drive, the ten spacing tokens and the header padding.
    const GEOMETRY_TOKENS: &[&str] = &[
        "radius_0",
        "radius_xs",
        "radius_s",
        "radius_m",
        "radius_l",
        "radius_xl",
        "radius_sm",
        "radius",
        "radius_lg",
        "field_radius",
        "menu_highlight_radius",
        "radius_pill",
        "spacing_none",
        "spacing_xxxs",
        "spacing_xxs",
        "spacing_xs",
        "spacing_s",
        "spacing_m",
        "spacing_l",
        "spacing_xl",
        "spacing_xxl",
        "spacing_xxxl",
        "header_padding_top",
        "header_padding_right",
        "header_padding_bottom",
        "header_padding_left",
    ];

    /// The tokens of `a` and `b` that differ, by name.
    fn changed(a: design::ResolvedStyle, b: design::ResolvedStyle) -> Vec<String> {
        let (a, b) = (a.values(), b.values());
        a.keys()
            .filter(|k| a.get(*k) != b.get(*k))
            .cloned()
            .collect()
    }

    /// Under an authored scheme at the defaults (accent `auto`, `round`,
    /// `comfortable`), for every style, the look draws today's design, and
    /// today's style but for geometry tokens only (F2: "the same look
    /// except corner radii"), with the radii exactly the `round` preset.
    #[test]
    fn an_authored_scheme_at_the_defaults_is_today_s_theme_but_for_radii() {
        let [r0, xs, s, m, l, xl] = design::geometry::Roundness::Round.radii();
        let styles = std::iter::once(None).chain(Style::ALL.map(Some));
        for style in styles {
            for scheme in Scheme::AUTHORED {
                let mut a = appearance(scheme.name());
                a.style = style.map(|s| s.name().to_owned());
                let today = theme(&a).unwrap();
                let mut changes: Vec<(&str, serde_json::Value)> =
                    vec![("appearance.scheme", scheme.name().into())];
                if let Some(style) = style {
                    changes.push(("appearance.style", style.name().into()));
                }
                let next = look(&look_of(&changes), None).expect("drawable");
                let case = format!("{scheme:?} {style:?}");
                assert_eq!(next.dictionary(), today.dictionary(), "{case}");
                assert_eq!(next.accent(), today.accent(), "{case}");
                let (old, new) = (today.style(), next.style());
                for token in changed(old, new) {
                    assert!(GEOMETRY_TOKENS.contains(&token.as_str()), "{case}: {token}");
                }
                assert_eq!(
                    [
                        new.radius_0,
                        new.radius_xs,
                        new.radius_s,
                        new.radius_m,
                        new.radius_l,
                        new.radius_xl
                    ],
                    [r0, xs, s, m, l, xl],
                    "{case}"
                );
                assert_eq!(
                    (
                        new.radius_sm,
                        new.radius,
                        new.radius_lg,
                        new.field_radius,
                        new.menu_highlight_radius,
                        new.radius_pill
                    ),
                    (xs, s, m, s, s, xl),
                    "{case}"
                );
            }
        }
    }

    /// The geometry overlay sets no legacy length at any density: those
    /// scale once, by `ui_scale`'s multiplier.
    #[test]
    fn density_scales_the_legacy_lengths_once() {
        for density in design::geometry::Density::ALL {
            let l = look_of(&[("appearance.density", density.name().into())]);
            let theme = look(&l, None).unwrap();
            for token in theme.overlay().style.keys() {
                assert!(
                    GEOMETRY_TOKENS.contains(&token.as_str()),
                    "{density:?}: {token}"
                );
            }
            let plain = look(&look_of(&[]), None).unwrap();
            assert_eq!(
                theme.style().field_height,
                plain.raw_style().field_height * density.multiplier(),
                "{density:?}"
            );
        }
    }

    /// The app axis reaches the compile context, as resolution passes it.
    #[test]
    fn the_app_reaches_the_design_context() {
        let l = look_of(&[]);
        let term = look(&l, Some("term")).unwrap();
        assert_eq!(term.context().app.as_deref(), Some("term"));
        assert_eq!(look(&l, None).unwrap().context().app, None);
        // The embedded design claims no app axis, so the app compiles as
        // none (a design that claims it is a source of its own: not drawn).
        assert_eq!(term.dictionary(), look(&l, None).unwrap().dictionary());
    }

    /// A tune the style refuses costs only the tunes, never the geometry or
    /// the fonts; one with no token form is skipped.
    #[test]
    fn a_bad_tune_is_dropped_not_the_look() {
        let mut l = look_of(&[
            ("appearance.roundness", "square".into()),
            ("appearance.font_weight", 700.into()),
        ]);
        l.tune
            .0
            .insert("style.radius".into(), "not a radius".into());
        l.tune
            .0
            .insert("style.widgets".into(), serde_json::json!([1]));
        let theme = look(&l, None).expect("still drawn");
        assert_eq!(theme.style().radius, 0, "square stands");
        assert_eq!(theme.overlay().weight, Some(700));
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

    /// A look the build cannot draw keeps the session's design and takes
    /// everything else the look says, as `look` would: framing, ui, every
    /// font choice, geometry and tunes, over the session's own style.
    #[test]
    fn a_look_with_its_own_source_keeps_the_session_design() {
        let source = design::EMBEDDED_DEFAULT_SOURCE.to_owned();
        let l = look_of(&[
            ("appearance.source", source.into()),
            ("window.decorations", "ssd".into()),
            ("appearance.font_weight", 700.into()),
            ("appearance.font_family", "Noto Sans".into()),
            ("appearance.icon_stroke", serde_json::json!(2.5)),
            ("appearance.roundness", "square".into()),
            ("appearance.density", "compact".into()),
            ("accessibility.text_scale", serde_json::json!(1.25)),
            ("accessibility.reduced_motion", true.into()),
            ("tune.style.radius_lg", 3.into()),
        ]);
        assert!(look(&l, None).is_none());
        let session = Theme::for_context(DesignContext {
            scheme: Scheme::Ocean,
            ..DesignContext::default()
        });
        let chosen = look_or(&l, None, &session);
        assert_eq!(chosen.dictionary(), session.dictionary());
        assert_eq!(chosen.decorations(), design::Decorations::Server);
        let overlay = chosen.overlay();
        assert_eq!(overlay.weight, Some(700));
        assert_eq!(overlay.family.as_deref(), Some("Noto Sans"));
        assert_eq!(overlay.stroke, Some(2.5));
        let ui = chosen.ui();
        assert_eq!(
            (ui.density, ui.text_scale, ui.reduced_motion),
            (0.85, 1.25, true)
        );
        let style = chosen.style();
        assert_eq!((style.radius, style.radius_sm, style.radius_lg), (0, 0, 3));
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
