// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The session look from today's delivery. `settings::follow` still hands an
//! app the v1 appearance and common UI (its `Followed::Look`); this adapter
//! turns them into the authored values the v2 views decode (key space v2
//! §4.1), so each app draws through [`settings::view::Look`] and
//! [`crate::session_look`]. S7 publishes the look topic and this adapter
//! goes: the follower then hands over the `Look` itself.
use crate::session_look;
use crate::theme::Theme;
use design::geometry::Roundness;
use design::{DesignContext, Scheme};
use serde_json::{Value, json};
use settings::model::{Appearance, CommonUi};
use settings::registry::Values;
use settings::view::{Look, View};

/// The theme today's delivery describes, drawn through [`Look`]. A profile
/// that names a design source of its own (`custom_source`) keeps the
/// session's design with its framing. The common UI applies on top, as the
/// shells always applied it.
pub fn theme(
    appearance: &Appearance,
    ui: &CommonUi,
    custom_source: bool,
    session: &Theme,
) -> Theme {
    let look = look(appearance, ui);
    let theme = if custom_source {
        session_look::framed_session(&look, session)
    } else {
        session_look::look_or(&look, None, session)
    };
    theme.with_ui(ui.clone())
}

/// The v2 look for a v1 delivery: the authored values of §4.1, decoded by
/// [`Look`] with its defaults filled.
pub fn look(appearance: &Appearance, ui: &CommonUi) -> Look {
    let mut values = Values::new();
    let mut set = |path: &str, value: Value| {
        values.insert(path.to_owned(), value);
    };
    set("appearance.scheme", json!(appearance.scheme));
    set("appearance.mode", json!(appearance.mode));
    set("appearance.contrast", json!(appearance.contrast));
    if let Some(style) = &appearance.style {
        set("appearance.style", json!(style));
    }
    set("appearance.roundness", json!(roundness(appearance).name()));
    set("appearance.density", json!(density(ui.density)));
    set("window.decorations", json!(appearance.decorations));
    set("window.caption_side", json!(appearance.caption_side));
    set("window.style", json!(appearance.window_style));
    set("window.menu_style", json!(appearance.menu_style));
    set("window.menu_side", json!(appearance.menu_side));
    set("window.actions_place", json!(appearance.actions_place));
    set("window.actions_align", json!(appearance.actions_align));
    set("window.title_align", json!(appearance.title_align));
    set("accessibility.text_scale", json!(ui.text_scale));
    set("accessibility.reduced_motion", json!(ui.reduced_motion));
    if let Some(stroke) = appearance.icon_stroke {
        set("appearance.icon_stroke", json!(stroke));
    }
    if let Some(family) = &appearance.font_family {
        set("appearance.font_family", json!(family));
    }
    if let Some(weight) = appearance.font_weight {
        set("appearance.font_weight", json!(weight));
    }
    if let Some(size) = appearance.font_size {
        set("appearance.font_size", json!(size));
    }
    if let Some(family) = &appearance.mono_family {
        set("appearance.mono_family", json!(family));
    }
    Look::of(&values)
}

/// The roundness of the style's preset (§4.1, F2). A style left to the
/// scheme takes the preset that changes none of the scheme's own radii.
fn roundness(appearance: &Appearance) -> Roundness {
    let preset = match appearance.style.as_deref() {
        Some("studio" | "adwaita") => Some(Roundness::Round),
        Some("breeze" | "pro") => Some(Roundness::SlightlyRound),
        Some("classic" | "plain") => Some(Roundness::Square),
        _ => None,
    };
    preset.unwrap_or_else(|| own_roundness(&appearance.scheme))
}

fn own_roundness(scheme: &str) -> Roundness {
    let Some(scheme) = Scheme::from_name(scheme) else {
        return Roundness::Round;
    };
    let own = || {
        Theme::for_context(DesignContext {
            scheme,
            ..DesignContext::default()
        })
        .raw_style()
    };
    [
        Roundness::Round,
        Roundness::SlightlyRound,
        Roundness::Square,
    ]
    .into_iter()
    .find(|&r| design::geometry::round(own(), r).values() == own().values())
    .unwrap_or(Roundness::Round)
}

/// The density step of a multiplier (§4.1): below 0.925 compact, up to 1.1
/// comfortable, else spacious.
fn density(multiplier: f64) -> &'static str {
    if multiplier < 0.925 {
        "compact"
    } else if multiplier <= 1.1 {
        "comfortable"
    } else {
        "spacious"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn appearance(style: Option<&str>) -> Appearance {
        Appearance {
            style: style.map(str::to_owned),
            ..Appearance::default()
        }
    }

    #[test]
    fn the_steps_and_roundness_follow_section_4_1() {
        let ui = |density| CommonUi {
            density,
            ..CommonUi::default()
        };
        assert_eq!(density(ui(0.85).density), "compact");
        assert_eq!(density(ui(1.0).density), "comfortable");
        assert_eq!(density(ui(1.2).density), "spacious");
        for (style, preset) in [
            ("studio", Roundness::Round),
            ("adwaita", Roundness::Round),
            ("breeze", Roundness::SlightlyRound),
            ("pro", Roundness::SlightlyRound),
            ("classic", Roundness::Square),
            ("plain", Roundness::Square),
        ] {
            assert_eq!(roundness(&appearance(Some(style))), preset, "{style}");
        }
    }

    #[test]
    fn the_view_decodes_the_delivered_look() {
        let mut a = appearance(Some("breeze"));
        a.decorations = "ssd".into();
        a.window_style = "win11".into();
        let ui = CommonUi {
            text_scale: 1.5,
            reduced_motion: true,
            ..CommonUi::default()
        };
        let look = look(&a, &ui);
        assert_eq!(look.appearance.scheme, "studio");
        assert_eq!(look.appearance.roundness, "slightly_round");
        assert_eq!(look.framing.decorations, "ssd");
        assert_eq!(look.framing.style, "win11");
        assert_eq!(look.text_scale, 1.5);
        assert!(look.reduced_motion);
    }

    #[test]
    fn the_common_styles_draw_the_same_theme_as_the_follower() {
        let session = Theme::embedded();
        for style in [None, Some("breeze"), Some("classic")] {
            let a = appearance(style);
            let ui = CommonUi::default();
            let old = session_look::follow(&a, false, &session).with_ui(ui.clone());
            let new = theme(&a, &ui, false, &session);
            assert_eq!(new.dictionary(), old.dictionary(), "{style:?}");
        }
    }
}
