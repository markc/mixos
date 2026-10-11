// SPDX-License-Identifier: MIT OR Apache-2.0
//! The session's look as settingsd holds it, turned into a [`Theme`]. Pure:
//! no Bus and no I/O. The app reads the desktop profile (for example with
//! `settings::follow::Follower`) and hands its appearance here.
//!
//! The appearance names the embedded design's scheme, style, mode and
//! contrast, and the framing (decorations, caption side). When it names a
//! design package of its own (`source`), or a name this build does not know,
//! the app's session theme stands, so a newer or custom desktop never
//! breaks an older app.
use crate::theme::Theme;
use design::{CaptionSide, Contrast, Decorations, DesignContext, Mode, Scheme, Style};
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
    let captions = CaptionSide::from_name(&appearance.caption_side)?;
    Some(Theme::for_context(context).framed(decorations, captions))
}

/// [`theme`], or `session` when the appearance cannot be drawn.
pub fn theme_or(appearance: &Appearance, session: &Theme) -> Theme {
    theme(appearance).unwrap_or_else(|| session.clone())
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
            source: None,
        };
        let theme = theme(&a).expect("drawable");
        assert_eq!(theme.scheme(), Scheme::Forest);
        assert_eq!(theme.style_axis(), Some(Style::Pro));
        assert_eq!(theme.mode(), Mode::Light);
        assert_eq!(theme.contrast(), Contrast::High);
        assert_eq!(theme.decorations(), Decorations::Server);
        assert_eq!(theme.captions(), CaptionSide::Left);
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
}
