// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The wallpaper from settingsd's `wallpaper.*` (ruling 6): backdrop's
//! authority once the store holds it. The view maps onto the same
//! `schema: 1` document that `backdrop.mx` holds, and that document is
//! validated by [`Config::from_document`], so a value means the same whichever
//! way it arrived.
//!
//! Mapping, per design §2.1 and F10:
//!
//! - a slideshow that is enabled and has a folder is the default fill;
//!   otherwise an image; otherwise a colour; otherwise the session theme;
//! - `fit` applies to every output, as does the colour;
//! - per-output images are used only while `same_on_all` is false, and an
//!   output without one takes the default;
//! - a gradient colour is drawn as its first stop: backdrop paints solid
//!   colours only.

use crate::fit::Fit;
use crate::schema::{Config, Document, Fill, Invalid, SCHEMA, Slideshow};
use settings::view::Wallpaper as View;
use std::collections::BTreeMap;

/// The configuration `wallpaper` names, validated like a file or Bus body.
pub fn config_from_view(view: &View) -> Result<Config, Invalid> {
    Config::from_document(document_from_view(view))
}

/// The document `wallpaper` says. It is not validated here; see
/// [`config_from_view`].
pub fn document_from_view(view: &View) -> Document {
    let fit = fit_of(&view.fit);
    let default = default_fill(view, fit);
    let outputs: BTreeMap<String, Fill> = if view.same_on_all {
        Default::default()
    } else {
        view.outputs
            .iter()
            .map(|(connector, image)| {
                (
                    connector.clone(),
                    Fill {
                        image: Some(image.clone()),
                        fit: fit_if_placed(fit),
                        ..Fill::default()
                    },
                )
            })
            .collect()
    };
    Document {
        schema: SCHEMA,
        default,
        outputs,
    }
}

/// The default fill, or `None` for the session theme.
fn default_fill(view: &View, fit: Fit) -> Option<Fill> {
    if view.slideshow.enabled
        && let Some(folder) = &view.slideshow.folder
    {
        return Some(Fill {
            slideshow: Some(Slideshow {
                folder: Some(folder.clone()),
                images: None,
                interval_s: u64::from(view.slideshow.interval_s),
                shuffle: view.slideshow.shuffle,
            }),
            fit: fit_if_placed(fit),
            ..Fill::default()
        });
    }
    if let Some(image) = &view.image {
        return Some(Fill {
            image: Some(image.clone()),
            fit: fit_if_placed(fit),
            ..Fill::default()
        });
    }
    view.color.as_deref().map(|colour| Fill {
        colour: Some(solid(colour).to_owned()),
        ..Fill::default()
    })
}

/// `fill`, `fit` or `stretch`; anything else is the default, `fill`.
fn fit_of(name: &str) -> Fit {
    match name {
        "fit" => Fit::Fit,
        "stretch" => Fit::Stretch,
        _ => Fit::Fill,
    }
}

/// A placement applies only to an image or slideshow (C2 in the schema), so
/// a colour fill carries none.
fn fit_if_placed(fit: Fit) -> Option<Fit> {
    (fit != Fit::Fill).then_some(fit)
}

/// The colour backdrop paints: the colour itself, or the first stop of a
/// `gradient:#a,#b@<deg>` value.
fn solid(colour: &str) -> &str {
    match colour.strip_prefix("gradient:") {
        Some(stops) => stops.split(',').next().unwrap_or(stops),
        None => colour,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Wallpaper;
    use serde_json::{Value, json};
    use settings::registry::{self, Values};

    /// The view of `changes` applied to the registry defaults.
    fn view(changes: &[(&str, Value)]) -> View {
        let changes: BTreeMap<String, Value> = changes
            .iter()
            .map(|(path, value)| ((*path).to_owned(), value.clone()))
            .collect();
        let values: Values =
            registry::apply(&Values::new(), &changes, &[]).unwrap_or_else(|e| panic!("{e:?}"));
        <View as settings::view::View>::of(&values)
    }

    #[test]
    fn the_registry_defaults_are_the_session_theme() {
        assert_eq!(
            config_from_view(&view(&[])).unwrap(),
            Config::default(),
            "no image, colour or slideshow means the theme"
        );
    }

    #[test]
    fn an_image_with_a_fit_is_the_default_fill() {
        let config = config_from_view(&view(&[
            ("wallpaper.image", json!("/srv/walls/a.png")),
            ("wallpaper.fit", json!("fit")),
        ]))
        .unwrap();
        assert_eq!(
            config.default,
            Wallpaper::Image {
                path: "/srv/walls/a.png".into(),
                fallback: None,
                fit: Fit::Fit,
            }
        );
    }

    #[test]
    fn a_colour_carries_no_placement_and_a_gradient_is_its_first_stop() {
        let solid = config_from_view(&view(&[("wallpaper.color", json!("#1E1E2E"))])).unwrap();
        assert_eq!(
            solid.default,
            Wallpaper::Colour(crate::colour::Rgb([0x1e, 0x1e, 0x2e]))
        );
        let gradient = config_from_view(&view(&[
            ("wallpaper.color", json!("gradient:#000000,#ffffff@90")),
            ("wallpaper.fit", json!("stretch")),
        ]))
        .unwrap();
        assert_eq!(
            gradient.default,
            Wallpaper::Colour(crate::colour::Rgb([0, 0, 0]))
        );
    }

    #[test]
    fn an_enabled_slideshow_with_a_folder_wins_over_the_image() {
        let config = config_from_view(&view(&[
            ("wallpaper.image", json!("/srv/walls/a.png")),
            ("wallpaper.slideshow.enabled", json!(true)),
            ("wallpaper.slideshow.folder", json!("/srv/walls")),
            ("wallpaper.slideshow.interval_s", json!(300)),
            ("wallpaper.slideshow.shuffle", json!(true)),
            ("wallpaper.fit", json!("fit")),
        ]))
        .unwrap();
        assert_eq!(
            config.default,
            Wallpaper::Slideshow {
                source: crate::schema::SlideSource::Folder("/srv/walls".into()),
                interval_s: 300,
                shuffle: true,
                fallback: None,
                fit: Fit::Fit,
            }
        );
    }

    #[test]
    fn a_disabled_slideshow_is_ignored() {
        let config = config_from_view(&view(&[
            ("wallpaper.slideshow.folder", json!("/srv/walls")),
            ("wallpaper.color", json!("#000000")),
        ]))
        .unwrap();
        assert_eq!(
            config.default,
            Wallpaper::Colour(crate::colour::Rgb([0; 3]))
        );
    }

    #[test]
    fn per_output_images_apply_only_while_the_outputs_differ() {
        let changes = vec![
            ("wallpaper.image", json!("/srv/walls/a.png")),
            ("wallpaper.outputs.DP-1.image", json!("/srv/walls/b.png")),
        ];
        let mut apart_changes = changes.clone();
        apart_changes.push(("wallpaper.same_on_all", json!(false)));
        let apart = config_from_view(&view(&apart_changes)).unwrap();
        assert_eq!(
            apart.for_output(Some("DP-1")),
            &Wallpaper::Image {
                path: "/srv/walls/b.png".into(),
                fallback: None,
                fit: Fit::Fill,
            }
        );
        assert_eq!(
            apart.for_output(Some("eDP-1")),
            &apart.default,
            "an output without one takes the shared image"
        );
        let shared = config_from_view(&view(&changes)).unwrap();
        assert!(shared.outputs.is_empty(), "same_on_all ignores the outputs");
        assert_eq!(shared.for_output(Some("DP-1")), &shared.default);
    }

    #[test]
    fn an_unusable_path_is_refused_by_the_document_rules() {
        // The registry refuses a relative path before a view exists, so the
        // view is built valid and its field changed by hand.
        let mut wallpaper = view(&[]);
        wallpaper.image = Some("relative.png".into());
        let error = config_from_view(&wallpaper).unwrap_err();
        assert!(error.to_string().contains("default.image"), "{error}");
    }
}
