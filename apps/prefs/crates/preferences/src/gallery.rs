// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Wallpaper chooser's state (spec 1 §4.1): the category it shows (the
//! system folder, Colors, a recent folder), the output tab selected, and
//! the images of the folder shown. Folder reads are the shell's
//! ([`Effect::Scan`]); everything a pick writes goes through the page's
//! control ([`crate::pages::wallpaper`]). The lists that outlive the window
//! (recent folders, custom images and colours) are settings, so every
//! desk's Preferences shows the same ones.
use crate::engine::{Effect, Engine};
use crate::pages::wallpaper::{
    COLOUR, COLOURS, CURRENT, CUSTOM_COLOURS, CUSTOM_IMAGES, IMAGE, RECENT, RECENT_MAX,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// At most this many images from one folder (COSMIC's bound).
pub const MAX_IMAGES: usize = 500;
/// Folders are read this deep.
pub const MAX_DEPTH: usize = 3;

/// What the chooser shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "folder")]
pub enum Category {
    /// The system wallpaper folder.
    Wallpapers,
    Colours,
    /// A folder the person opened (one of the recent folders).
    Folder(String),
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Gallery {
    /// Chosen in the menu this session; `None` follows the settings (a
    /// colour shows Colors, a current folder that folder).
    pub category: Option<Category>,
    /// The system wallpaper folder, as the shell finds it.
    pub system: Option<String>,
    /// The folder `images` were read from.
    pub scanned: Option<String>,
    #[serde(skip)]
    scanned_at: Option<std::time::Instant>,
    pub images: Vec<String>,
    pub truncated: bool,
    /// The read in flight: its ticket and folder.
    #[serde(skip)]
    scanning: Option<(u64, String)>,
    pub error: Option<String>,
}

impl Engine {
    /// The category shown.
    pub fn category(&self) -> Category {
        if let Some(category) = &self.gallery.category {
            return category.clone();
        }
        let set = |key: &str| self.store.value(key).is_some_and(|v| !v.is_null());
        if set(COLOUR) && !set(IMAGE) {
            return Category::Colours;
        }
        match self.store.text(CURRENT) {
            Some(folder) => Category::Folder(folder),
            None => Category::Wallpapers,
        }
    }

    /// The folder the chooser shows, if it shows one.
    pub fn gallery_folder(&self) -> Option<String> {
        match self.category() {
            Category::Wallpapers => self.gallery.system.clone(),
            Category::Folder(folder) => Some(folder),
            Category::Colours => None,
        }
    }

    fn list(&self, key: &str) -> Vec<String> {
        self.store
            .value(key)
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    }

    pub fn recent_folders(&self) -> Vec<String> {
        self.list(RECENT)
    }

    pub fn custom_images(&self) -> Vec<String> {
        self.list(CUSTOM_IMAGES)
    }

    pub fn custom_colours(&self) -> Vec<String> {
        self.list(CUSTOM_COLOURS)
    }

    /// The images the grid shows: the custom ones first (on the system
    /// folder), then the folder's.
    pub fn gallery_images(&self) -> Vec<String> {
        let mut images = match self.category() {
            Category::Wallpapers => self.custom_images(),
            _ => Vec::new(),
        };
        if self.gallery.scanned == self.gallery_folder() {
            for image in &self.gallery.images {
                if !images.contains(image) {
                    images.push(image.clone());
                }
            }
        }
        images
    }

    /// The colours the grid shows: the custom ones, then the built-in.
    pub fn gallery_colours(&self) -> Vec<String> {
        let mut colours = self.custom_colours();
        colours.extend(COLOURS.iter().map(|c| (*c).to_owned()));
        colours
    }

    /// Read the shown folder, unless it is read already or being read.
    pub fn scan_gallery(&mut self) {
        let Some(folder) = self.gallery_folder() else {
            return;
        };
        let fresh = self.gallery.scanned.as_ref() == Some(&folder);
        let reading = self.gallery.scanning.as_ref().map(|s| &s.1) == Some(&folder);
        if fresh || reading || self.quitting {
            return;
        }
        let ticket = self.next();
        self.gallery.scanning = Some((ticket, folder.clone()));
        self.effects.push(Effect::Scan { ticket, folder });
    }

    /// Reuse a recent successful scan on page entry; retry errors on entry.
    pub fn enter_gallery(&mut self) {
        if self.gallery.error.is_some()
            || self
                .gallery
                .scanned_at
                .is_none_or(|at| at.elapsed().as_secs() >= 60)
        {
            self.rescan_gallery();
        } else {
            self.scan_gallery();
        }
    }

    /// Explicitly read the shown folder again.
    pub fn rescan_gallery(&mut self) {
        self.gallery.scanned = None;
        self.scan_gallery();
    }

    /// The read `ticket` finished. Returns whether it was accepted.
    pub fn gallery_scanned(&mut self, ticket: u64, result: Result<Vec<String>, String>) -> bool {
        let Some((expected, folder)) = self.gallery.scanning.take() else {
            return false;
        };
        if expected != ticket {
            self.gallery.scanning = Some((expected, folder));
            return false;
        }
        match result {
            Ok(images) => {
                self.gallery.images = images;
                self.gallery.error = None;
            }
            Err(error) => {
                self.gallery.images.clear();
                self.gallery.error = Some(error);
            }
        }
        self.gallery.scanned = Some(folder);
        self.gallery.scanned_at = Some(std::time::Instant::now());
        // The category moved on while this one was read.
        self.scan_gallery();
        true
    }

    /// Show `category`. A folder becomes the current one (a slideshow runs
    /// through it); the system folder clears it.
    pub fn set_category(&mut self, category: Category) -> Result<(), String> {
        let current = match &category {
            Category::Wallpapers => Some(Value::Null),
            Category::Folder(folder) => Some(json!(folder)),
            Category::Colours => None,
        };
        if let Some(current) = current
            && self.store.value(CURRENT).unwrap_or(Value::Null) != current
        {
            self.set_key(CURRENT, current)?;
        }
        self.gallery.category = Some(category);
        self.scan_gallery();
        Ok(())
    }

    /// A folder chosen with "Open new folder": first among the recent ones
    /// (at most five) and the one shown.
    pub fn open_folder(&mut self, folder: String) -> Result<(), String> {
        let mut recent = self.recent_folders();
        recent.retain(|f| f != &folder);
        recent.insert(0, folder.clone());
        recent.truncate(RECENT_MAX);
        self.set_keys(BTreeMap::from([
            (RECENT.to_owned(), json!(recent)),
            (CURRENT.to_owned(), json!(folder)),
        ]))?;
        self.gallery.category = Some(Category::Folder(folder));
        self.scan_gallery();
        Ok(())
    }

    /// Images chosen with "Add image": remembered first among the custom
    /// ones, and the first of them shown, as a pick of it would.
    pub fn add_images(&mut self, images: Vec<String>) -> Result<(), String> {
        let Some(first) = images.first().cloned() else {
            return Ok(());
        };
        let mut custom = self.custom_images();
        custom.retain(|i| !images.contains(i));
        let mut added = images;
        added.extend(custom);
        added.truncate(64);
        let mut changes = self.control_writes(IMAGE, json!(first));
        changes.insert(CUSTOM_IMAGES.into(), json!(added));
        self.set_keys(changes)
    }

    /// Forget a custom image or colour (the system folder's own images and
    /// the built-in colours have no remove button).
    pub fn remove_custom(&mut self, key: &str, value: &str) -> Result<(), String> {
        if key != CUSTOM_IMAGES && key != CUSTOM_COLOURS {
            return Err(self.label("set-no-control", &[("key", key)]));
        }
        let mut list = self.list(key);
        let before = list.len();
        list.retain(|v| v != value);
        if list.len() == before {
            return Ok(());
        }
        self.set_key(key, json!(list))
    }

    /// A folder is being read.
    pub fn gallery_reading(&self) -> bool {
        self.gallery.scanning.is_some()
    }

    /// The chooser as data: what it shows, and what a pick may write.
    pub fn gallery_view(&self) -> Value {
        json!({"category": self.category(), "folder": self.gallery_folder(),
            "output": self.selected_output(), "recent": self.recent_folders(),
            "images": self.gallery_images(), "colours": self.gallery_colours(),
            "custom_images": self.custom_images(), "custom_colours": self.custom_colours(),
            "reading": self.gallery.scanning.is_some(), "error": self.gallery.error,
            "truncated": self.gallery.truncated})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_stop_until_reentry_and_success_is_reused() {
        let mut e = engine();
        e.scan_gallery();
        let Effect::Scan { ticket, .. } = e.take_effects()[0] else {
            panic!()
        };
        e.gallery_scanned(ticket, Err("missing".into()));
        e.scan_gallery();
        assert!(e.take_effects().is_empty());
        e.enter_gallery();
        let Effect::Scan { ticket, .. } = e.take_effects()[0] else {
            panic!()
        };
        e.gallery_scanned(ticket, Ok(vec![]));
        e.open_page("appearance", None).unwrap();
        e.open_page("wallpaper", None).unwrap();
        assert!(
            !e.take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Scan { .. }))
        );
        e.gallery.scanned_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(61));
        e.enter_gallery();
        assert!(
            e.take_effects()
                .iter()
                .any(|e| matches!(e, Effect::Scan { .. }))
        );
    }

    fn engine() -> Engine {
        let mut e = Engine::new(|key, _| key.to_owned());
        e.store_read(&json!({"incarnation":"inc-1","revision":"7","values":{}}));
        e.store_status(&json!({"role":"authority","stale":false}));
        e.gallery.system = Some("/usr/share/wallpapers".into());
        e.take_effects();
        e
    }

    #[test]
    fn a_folder_is_read_once_and_a_late_read_is_dropped() {
        let mut e = engine();
        e.scan_gallery();
        e.scan_gallery();
        let effects = e.take_effects();
        assert_eq!(effects.len(), 1);
        let Effect::Scan { ticket, folder } = &effects[0] else {
            panic!("{effects:?}");
        };
        assert_eq!(folder, "/usr/share/wallpapers");
        e.gallery_scanned(ticket + 1, Ok(vec!["/elsewhere.png".into()]));
        assert!(e.gallery_images().is_empty());
        e.gallery_scanned(*ticket, Ok(vec!["/usr/share/wallpapers/a.png".into()]));
        assert_eq!(e.gallery_images(), ["/usr/share/wallpapers/a.png"]);
    }

    #[test]
    fn opening_a_folder_keeps_five_recent_and_shows_it() {
        let mut e = engine();
        e.store
            .values
            .insert(RECENT.into(), json!(["/a", "/b", "/c", "/d", "/e"]));
        e.open_folder("/f".into()).unwrap();
        assert_eq!(e.recent_folders(), ["/f", "/a", "/b", "/c", "/d"]);
        assert_eq!(e.category(), Category::Folder("/f".into()));
        assert_eq!(e.store.text(CURRENT).as_deref(), Some("/f"));
    }

    #[test]
    fn a_stored_colour_opens_on_colours() {
        let mut e = engine();
        e.store.values.insert(COLOUR.into(), json!("#000000"));
        assert_eq!(e.category(), Category::Colours);
        assert_eq!(e.gallery_colours().len(), COLOURS.len());
    }
}
