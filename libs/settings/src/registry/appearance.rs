// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.2 `appearance.*` (Desktop › Appearance).
use super::{Alpha, DefaultValue as D, Domain, Domain::*, Grammar, KeyDef as K, Kind, Kind::*};

const PAGE: &str = "desktop/appearance";
const EXPERIMENTAL: &str = "desktop/appearance/experimental";
const LOOK: &[Domain] = &[Paint, Text, Layout, Resources];

const fn k(path: &'static str, kind: Kind, default: D, domains: &'static [Domain]) -> K {
    K::new(path, kind, default).on(PAGE, domains)
}
const fn frosted(path: &'static str) -> K {
    k(path, Bool, D::Bool(false), &[Paint])
        .used_when("appearance.frosted.thickness is above 0")
        .constraints(&["C1"])
}

/// Today's eleven schemes and the derived `mixos`.
pub const SCHEMES: &[&str] = &[
    "mixos", "ocean", "crimson", "stone", "forest", "sunset", "mono", "pro", "studio", "classic",
    "breeze", "adwaita",
];
pub const STYLES: &[&str] = &["plain", "pro", "studio", "classic", "breeze", "adwaita"];
/// `auto` and COSMIC's nine accent swatches; each swatch resolves per mode
/// in `libs/design`.
const ACCENT_WORDS: &[&str] = &[
    "auto",
    "blue",
    "indigo",
    "purple",
    "pink",
    "red",
    "orange",
    "yellow",
    "green",
    "warm_grey",
];

pub(super) const KEYS: &[K] = &[
    k("appearance.mode", Enum(&["dark", "light", "auto"]), D::Text("dark"), LOOK),
    k("appearance.scheme", Enum(SCHEMES), D::Text("mixos"), LOOK).mixos(),
    k("appearance.style", Enum(STYLES), D::Null, LOOK).mixos(),
    k(
        "appearance.accent",
        ColourOr(ACCENT_WORDS, Alpha::Opaque),
        D::Text("auto"),
        &[Paint],
    ),
    k(
        "appearance.window_bg",
        ColourOr(&["auto"], Alpha::Any),
        D::Text("auto"),
        &[Paint],
    ),
    k(
        "appearance.container_bg",
        ColourOr(&["auto"], Alpha::Any),
        D::Text("auto"),
        &[Paint],
    ),
    k(
        "appearance.text_tint",
        ColourOr(&["auto"], Alpha::Opaque),
        D::Text("auto"),
        &[Paint],
    ),
    k(
        "appearance.control_tint",
        ColourOr(&["auto"], Alpha::Opaque),
        D::Text("auto"),
        &[Paint],
    ),
    k(
        "appearance.contrast",
        Enum(&["normal", "high"]),
        D::Text("normal"),
        &[Paint],
    ),
    k(
        "appearance.roundness",
        Enum(&["round", "slightly_round", "square"]),
        D::Text("round"),
        &[Layout, Paint],
    ),
    k(
        "appearance.density",
        Enum(&["compact", "comfortable", "spacious"]),
        D::Text("comfortable"),
        &[Layout],
    ),
    k(
        "appearance.frosted.thickness",
        Int {
            min: 0,
            max: 13,
            step: 1,
        },
        D::Int(6),
        &[Paint],
    ),
    k(
        "appearance.frosted.opacity",
        Number {
            min: 0.0,
            max: 1.0,
            step: 0.05,
        },
        D::Number(0.5),
        &[Paint],
    ),
    frosted("appearance.frosted.panel"),
    frosted("appearance.frosted.applets"),
    frosted("appearance.frosted.system"),
    frosted("appearance.frosted.windows"),
    k(
        "appearance.font_family",
        Str(Grammar::Plain, 128),
        D::Null,
        &[Text, Layout],
    ),
    k(
        "appearance.font_weight",
        Int {
            min: 100,
            max: 900,
            step: 100,
        },
        D::Null,
        &[Text],
    )
    .page(EXPERIMENTAL)
    .mixos(),
    k(
        "appearance.font_size",
        Number {
            min: crate::model::FONT_SIZE_MIN,
            max: crate::model::FONT_SIZE_MAX,
            step: 0.0,
        },
        D::Null,
        &[Text, Layout],
    )
    .page(EXPERIMENTAL)
    .mixos(),
    k(
        "appearance.mono_family",
        Str(Grammar::Plain, 128),
        D::Null,
        &[Text],
    ),
    k(
        "appearance.icon_stroke",
        Number {
            min: crate::model::ICON_STROKE_MIN,
            max: crate::model::ICON_STROKE_MAX,
            step: crate::model::ICON_STROKE_STEP,
        },
        D::Null,
        &[Paint],
    )
    .page(EXPERIMENTAL)
    .mixos(),
    k(
        "appearance.icon_theme",
        Str(Grammar::DirName, 64),
        D::Text("breeze"),
        &[Export],
    )
    .export_only(),
    k(
        "appearance.accent_palette",
        List(&Colour(Alpha::Opaque), 9, 9),
        D::Null,
        &[],
    )
    .mixos(),
    k(
        "appearance.export_toolkits",
        Bool,
        D::Bool(true),
        &[Export],
    ),
    k("appearance.export_desktop", Bool, D::Bool(true), &[Export]),
    k(
        "appearance.source",
        Str(Grammar::Source, crate::MAX_SOURCE_BYTES),
        D::Null,
        LOOK,
    )
    .mixos(),
];
