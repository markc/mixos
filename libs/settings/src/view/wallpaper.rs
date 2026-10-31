// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `wallpaper.*`: backdrop's authority (ruling 6), and Prefs' own lists.
use super::{Reader, View, family};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Wallpaper {
    pub same_on_all: bool,
    /// An absolute image path; None is the theme colour.
    pub image: Option<String>,
    /// Per-output images by connector, used while `same_on_all` is false
    /// (C4). An output without one falls back to `image`.
    pub outputs: BTreeMap<String, String>,
    /// `#rrggbb` or `gradient:#a,#b@<deg>`, for every output.
    pub color: Option<String>,
    /// `fill`, `fit` or `stretch`, for every output.
    pub fit: String,
    pub slideshow: Slideshow,
    pub folders: Folders,
    pub custom_images: Vec<String>,
    pub custom_colors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Slideshow {
    pub enabled: bool,
    pub folder: Option<String>,
    pub interval_s: u32,
    pub shuffle: bool,
}

/// Prefs' browsing state; no consumer renders it.
#[derive(Clone, Debug, PartialEq)]
pub struct Folders {
    pub current: Option<String>,
    pub recent: Vec<String>,
}

impl Wallpaper {
    /// The image for `connector`: its own while the outputs differ, else the
    /// shared one.
    pub fn image_for(&self, connector: &str) -> Option<&str> {
        if !self.same_on_all
            && let Some(image) = self.outputs.get(connector)
        {
            return Some(image);
        }
        self.image.as_deref()
    }
}

impl View for Wallpaper {
    fn read(r: &Reader) -> Self {
        let outputs = family(r, "wallpaper.outputs.", 2, |o| {
            r.opt_text(&format!("wallpaper.outputs.{o}.image"))
        })
        .into_iter()
        .filter_map(|(o, image)| Some((o, image?)))
        .collect();
        Self {
            same_on_all: r.bool("wallpaper.same_on_all"),
            image: r.opt_text("wallpaper.image"),
            outputs,
            color: r.opt_text("wallpaper.color"),
            fit: r.text("wallpaper.fit"),
            slideshow: Slideshow {
                enabled: r.bool("wallpaper.slideshow.enabled"),
                folder: r.opt_text("wallpaper.slideshow.folder"),
                interval_s: r.uint("wallpaper.slideshow.interval_s"),
                shuffle: r.bool("wallpaper.slideshow.shuffle"),
            },
            folders: Folders {
                current: r.opt_text("wallpaper.folders.current"),
                recent: r.texts("wallpaper.folders.recent"),
            },
            custom_images: r.texts("wallpaper.custom_images"),
            custom_colors: r.texts("wallpaper.custom_colors"),
        }
    }
}
