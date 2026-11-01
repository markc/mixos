// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Desktop › Wallpaper (spec 1 §4.1; the matrix §1) over `wallpaper.*`,
//! which backdrop follows. The preview and the chooser sit on the page; the
//! options are one card. What a control sends with its key is here
//! ([`super::Writes`]): backdrop draws a slideshow before an image before a
//! colour, so choosing one clears what would hide it.
use super::{Body, ChoiceStyle, Cond, Control, Opt, Page, opt, opt_int, row, unlisted, untitled};
use crate::engine::Engine;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const SAME: &str = "wallpaper.same_on_all";
pub const IMAGE: &str = "wallpaper.image";
pub const COLOUR: &str = "wallpaper.color";
pub const FIT: &str = "wallpaper.fit";
pub const SLIDESHOW: &str = "wallpaper.slideshow.enabled";
pub const FOLDER: &str = "wallpaper.slideshow.folder";
pub const CURRENT: &str = "wallpaper.folders.current";
pub const RECENT: &str = "wallpaper.folders.recent";
pub const CUSTOM_IMAGES: &str = "wallpaper.custom_images";
pub const CUSTOM_COLOURS: &str = "wallpaper.custom_colors";
/// At most this many recent folders (the registry's bound).
pub const RECENT_MAX: usize = 5;

/// The per-output image key of `connector`.
pub fn output_image(connector: &str) -> String {
    format!("wallpaper.outputs.{connector}.image")
}

/// The built-in choices after the custom colours: fourteen flat colours,
/// then five two-stop gradients drawn top to bottom.
pub const COLOURS: &[&str] = &[
    "#000000",
    "#262626",
    "#4d4d4d",
    "#808080",
    "#d9d9d9",
    "#1e3a5f",
    "#2f6f9f",
    "#2e7d6b",
    "#4f7d2e",
    "#8a7a2a",
    "#a0522d",
    "#8b2e3c",
    "#6a3d7c",
    "#3d3a7c",
    "gradient:#1e3a5f,#6a3d7c@180",
    "gradient:#2e7d6b,#1e3a5f@180",
    "gradient:#a0522d,#8b2e3c@180",
    "gradient:#262626,#4d4d4d@180",
    "gradient:#8a7a2a,#2e7d6b@180",
];

const FITS: &[Opt] = &[
    opt("fill", "opt-fill"),
    opt("fit", "opt-fit-screen"),
    unlisted("stretch", "opt-fill"),
];
const INTERVALS: &[Opt] = &[
    opt_int(300, "opt-minutes-5"),
    opt_int(600, "opt-minutes-10"),
    opt_int(900, "opt-minutes-15"),
    opt_int(1800, "opt-minutes-30"),
    opt_int(3600, "opt-hours-1"),
    opt_int(7200, "opt-hours-2"),
];

pub(super) fn page() -> Page {
    let sections = vec![
        untitled(
            "screen",
            vec![
                row(
                    "preview",
                    Control::Preview {
                        image: IMAGE,
                        colour: COLOUR,
                        fit: FIT,
                    },
                ),
                row("display", Control::Outputs { all: SAME }),
            ],
        )
        .plain(),
        untitled(
            "options",
            vec![
                row("same_on_all", Control::Toggle { key: SAME }).writes(same_on_all),
                row(
                    "fit",
                    Control::Choice {
                        key: FIT,
                        style: ChoiceStyle::Dropdown,
                        options: FITS,
                    },
                ),
                row("slideshow", Control::Toggle { key: SLIDESHOW })
                    .show(Cond::Unset(COLOUR))
                    .writes(slideshow),
                row(
                    "interval",
                    Control::Choice {
                        key: "wallpaper.slideshow.interval_s",
                        style: ChoiceStyle::Dropdown,
                        options: INTERVALS,
                    },
                )
                .show(Cond::True(SLIDESHOW)),
                row(
                    "shuffle",
                    Control::Toggle {
                        key: "wallpaper.slideshow.shuffle",
                    },
                )
                .show(Cond::True(SLIDESHOW)),
            ],
        ),
        untitled(
            "choose",
            vec![
                row(
                    "gallery",
                    Control::Gallery {
                        image: IMAGE,
                        colour: COLOUR,
                    },
                )
                .writes(gallery),
            ],
        )
        .plain(),
    ];
    super::page(
        "wallpaper",
        "image",
        Some("desktop"),
        Body::Sections {
            sections,
            drawers: Vec::new(),
        },
    )
}

fn set(e: &Engine, key: &str) -> bool {
    e.store.value(key).is_some_and(|v| !v.is_null())
}

fn texts(e: &Engine, key: &str) -> Vec<String> {
    e.store
        .value(key)
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// Turning "same on all" on clears every per-output image (COSMIC keeps
/// one entry for all outputs).
fn same_on_all(e: &Engine, key: &str, value: &Value) -> BTreeMap<String, Value> {
    let mut changes = BTreeMap::from([(key.to_owned(), value.clone())]);
    if value.as_bool() == Some(true) {
        for path in e.store.values.keys() {
            if path.starts_with("wallpaper.outputs.") && path.ends_with(".image") {
                changes.insert(path.clone(), Value::Null);
            }
        }
    }
    changes
}

/// On, the slideshow runs through the folder the chooser shows (else its
/// own, the current one, the system one); a slideshow needs a folder (C3).
/// Off, the stored image shows again, or the first one of the chooser.
fn slideshow(e: &Engine, key: &str, value: &Value) -> BTreeMap<String, Value> {
    let mut changes = BTreeMap::from([(key.to_owned(), value.clone())]);
    if value.as_bool() == Some(true) {
        let folder = e
            .gallery_folder()
            .or_else(|| e.store.text(FOLDER))
            .or_else(|| e.store.text(CURRENT))
            .or_else(|| e.gallery.system.clone());
        if let Some(folder) = folder {
            changes.insert(FOLDER.into(), json!(folder));
        }
    } else if !set(e, IMAGE)
        && let Some(first) = e.gallery_images().first()
    {
        changes.insert(IMAGE.into(), json!(first));
    }
    changes
}

/// Choosing an image clears the colour and stops the slideshow; with the
/// outputs apart, it is the selected output's own. Choosing a colour
/// clears the image and stops the slideshow, and a colour the grid does
/// not offer is remembered among the custom ones.
fn gallery(e: &Engine, key: &str, value: &Value) -> BTreeMap<String, Value> {
    let mut changes = BTreeMap::new();
    if key == IMAGE
        && !e.store.flag(SAME)
        && let Some(output) = e.selected_output()
    {
        changes.insert(output_image(&output), value.clone());
        return changes;
    }
    changes.insert(key.to_owned(), value.clone());
    if value.is_null() {
        return changes;
    }
    let other = if key == IMAGE { COLOUR } else { IMAGE };
    if set(e, other) {
        changes.insert(other.into(), Value::Null);
    }
    if e.store.flag(SLIDESHOW) {
        changes.insert(SLIDESHOW.into(), json!(false));
    }
    if key == COLOUR
        && let Some(colour) = value.as_str()
    {
        let mut custom = texts(e, CUSTOM_COLOURS);
        let colour = colour.to_lowercase();
        if !COLOURS.contains(&colour.as_str()) && !custom.contains(&colour) {
            custom.insert(0, colour);
            changes.insert(CUSTOM_COLOURS.into(), json!(custom));
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(pairs: &[(&str, Value)]) -> Engine {
        let values: serde_json::Map<String, Value> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        let mut e = Engine::new(|key, _| key.to_owned());
        e.store_read(&json!({"incarnation":"inc-1","revision":"7","values":values}));
        e.store_status(&json!({"role":"authority","stale":false}));
        e.take_effects();
        e
    }

    #[test]
    fn the_builtin_colours_are_wallpaper_colours() {
        assert_eq!(COLOURS.len(), 19);
        for colour in COLOURS {
            assert!(
                settings::registry::check_value(COLOUR, &json!(colour)).is_ok(),
                "{colour}"
            );
        }
    }

    #[test]
    fn an_image_clears_the_colour_and_stops_the_slideshow() {
        let e = engine(&[
            (COLOUR, json!("#000000")),
            (SLIDESHOW, json!(true)),
            (FOLDER, json!("/srv/walls")),
        ]);
        assert_eq!(
            gallery(&e, IMAGE, &json!("/srv/walls/a.png")),
            BTreeMap::from([
                (IMAGE.to_owned(), json!("/srv/walls/a.png")),
                (COLOUR.to_owned(), Value::Null),
                (SLIDESHOW.to_owned(), json!(false)),
            ])
        );
    }

    #[test]
    fn a_new_colour_is_remembered_and_a_builtin_one_is_not() {
        let e = engine(&[(IMAGE, json!("/srv/walls/a.png"))]);
        let changes = gallery(&e, COLOUR, &json!("#123456"));
        assert_eq!(changes[IMAGE], Value::Null);
        assert_eq!(changes[CUSTOM_COLOURS], json!(["#123456"]));
        assert!(!gallery(&e, COLOUR, &json!(COLOURS[3])).contains_key(CUSTOM_COLOURS));
    }

    #[test]
    fn apart_an_image_is_the_selected_output_s() {
        let mut e = engine(&[(SAME, json!(false))]);
        e.set_options(
            "outputs",
            vec![
                (json!("DP-1"), "DP-1".into()),
                (json!("DP-2"), "DP-2".into()),
            ],
        );
        e.select_output(Some("DP-2".into()));
        assert_eq!(
            gallery(&e, IMAGE, &json!("/srv/walls/b.png")),
            BTreeMap::from([(output_image("DP-2"), json!("/srv/walls/b.png"))])
        );
    }

    #[test]
    fn same_on_all_clears_the_outputs_and_a_slideshow_names_its_folder() {
        let e = engine(&[
            (SAME, json!(false)),
            ("wallpaper.outputs.DP-1.image", json!("/srv/b.png")),
        ]);
        let changes = same_on_all(&e, SAME, &json!(true));
        assert_eq!(changes[&output_image("DP-1")], Value::Null);
        let mut e = engine(&[]);
        e.gallery.system = Some("/usr/share/wallpapers".into());
        assert_eq!(
            slideshow(&e, SLIDESHOW, &json!(true))[FOLDER],
            json!("/usr/share/wallpapers")
        );
    }
}
