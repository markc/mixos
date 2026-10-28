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
//! framing request, so a newer or custom desktop never breaks an older app.
use crate::theme::{Overlay, Theme};
use design::{
    CaptionSide, Contrast, DesignContext, Mode, RequestedDecorations as Decorations, Scheme, Style,
};
use settings::model::Appearance;

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
    theme(appearance).unwrap_or_else(|| {
        // Framing is independent of the design package: an unsupported
        // source must still follow the session's requested decorations.
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
            .with_overlay(Overlay::from_appearance(appearance))
            .expect("appearance overlays contain only validated type and icon choices")
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
        let session = Theme::embedded();
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
