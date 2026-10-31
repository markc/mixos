// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.1 `wallpaper.*` (Desktop › Wallpaper).
use super::{Alpha, DefaultValue as D, Domain::*, Grammar, KeyDef as K, Kind, Kind::*};

const fn k(path: &'static str, kind: Kind, default: D) -> K {
    K::new(path, kind, default).on("desktop/wallpaper", &[Shell])
}
const IMAGE: Kind = Path {
    directory: false,
    images: true,
};
const FOLDER: Kind = Path {
    directory: true,
    images: false,
};

pub(super) const KEYS: &[K] = &[
    k("wallpaper.same_on_all", Bool, D::Bool(true)),
    k("wallpaper.image", IMAGE, D::Null),
    k("wallpaper.outputs.<connector>.image", IMAGE, D::Null)
        .used_when("wallpaper.same_on_all is false")
        .constraints(&["C4"]),
    k(
        "wallpaper.color",
        Str(Grammar::WallpaperColour, 64),
        D::Null,
    ),
    k(
        "wallpaper.fit",
        Enum(&["fill", "fit", "stretch"]),
        D::Text("fill"),
    ),
    k("wallpaper.slideshow.enabled", Bool, D::Bool(false)).constraints(&["C3"]),
    k("wallpaper.slideshow.folder", FOLDER, D::Null).constraints(&["C3"]),
    k(
        "wallpaper.slideshow.interval_s",
        OneOf(&[300, 600, 900, 1800, 3600, 7200]),
        D::Int(600),
    ),
    k("wallpaper.slideshow.shuffle", Bool, D::Bool(false)).mixos(),
    k("wallpaper.folders.current", FOLDER, D::Null).domains(&[]),
    k(
        "wallpaper.folders.recent",
        List(&FOLDER, 0, 5),
        D::Json("[]"),
    )
    .domains(&[]),
    k(
        "wallpaper.custom_images",
        List(&IMAGE, 0, 64),
        D::Json("[]"),
    )
    .domains(&[]),
    k(
        "wallpaper.custom_colors",
        List(&Colour(Alpha::Opaque), 0, 32),
        D::Json("[]"),
    )
    .domains(&[]),
];
