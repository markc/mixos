// SPDX-License-Identifier: MIT OR Apache-2.0
//! Prefs' image browser state; directory I/O belongs to the shell.
use crate::{Effect, Engine};
use serde::Serialize;
use std::path::Path;

pub const MAX_ENTRIES: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub directory: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Listing {
    pub directory: String,
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Picker {
    pub open: bool,
    pub directory: String,
    pub entries: Vec<Entry>,
    pub selected: Option<String>,
    pub error: Option<String>,
    pub truncated: bool,
    pub loading: Option<u64>,
}

impl Engine {
    pub fn can_browse_wallpaper(&self) -> bool {
        self.can_edit_wallpaper() && !self.picker.open && self.picker.loading.is_none()
    }

    pub fn browse_wallpaper(&mut self) {
        if self.can_browse_wallpaper() {
            self.picker.open = true;
            self.list_wallpapers(None);
        }
    }

    fn list_wallpapers(&mut self, path: Option<String>) {
        if self.picker.loading.is_some() || self.quitting {
            return;
        }
        let ticket = self.next();
        self.picker.loading = Some(ticket);
        self.picker.error = None;
        self.picker.selected = None;
        self.effects.push(Effect::Browse { ticket, path });
    }

    pub fn edit_picker_directory(&mut self, path: String) {
        if self.picker.open && self.picker.loading.is_none() && !self.quitting {
            self.picker.directory = path;
        }
    }

    pub fn can_picker_go(&self) -> bool {
        self.picker.open && self.picker.loading.is_none() && !self.quitting
    }

    pub fn picker_go(&mut self) {
        if !self.can_picker_go() {
            return;
        }
        let path = self.picker.directory.clone();
        if !Path::new(&path).is_absolute() || path.contains('\0') || path.len() > 4096 {
            self.picker.error = Some(self.label("wallpaper-path-invalid", &[]));
            return;
        }
        self.list_wallpapers(Some(path));
    }

    pub fn picker_parent(&mut self) {
        if !self.can_picker_go() {
            return;
        }
        if let Some(parent) = Path::new(&self.picker.directory).parent()
            && let Some(path) = parent.to_str()
        {
            self.list_wallpapers(Some(path.to_owned()));
        }
    }

    pub fn picker_select(&mut self, path: &str) {
        if self.can_picker_go()
            && self.picker.entries.iter().any(|e| !e.directory && e.path == path)
        {
            self.picker.selected = Some(path.to_owned());
        }
    }

    pub fn picker_enter(&mut self, path: &str) {
        if self.can_picker_go()
            && self.picker.entries.iter().any(|e| e.directory && e.path == path)
        {
            self.list_wallpapers(Some(path.to_owned()));
        }
    }

    pub fn can_picker_use(&self) -> bool {
        self.can_picker_go() && self.picker.selected.is_some()
    }

    pub fn picker_use(&mut self) {
        if self.can_picker_use() {
            if let Some(path) = self.picker.selected.clone() {
                self.picker_cancel();
                self.wallpaper_kind(crate::wallpaper::Kind::Image);
                self.wallpaper_image(path);
            }
            self.picker_cancel();
        }
    }

    pub fn picker_cancel(&mut self) {
        self.picker.open = false;
        self.picker.selected = None;
    }

    pub fn browsed(&mut self, ticket: u64, result: Result<Listing, String>) {
        if self.picker.loading != Some(ticket) {
            return;
        }
        self.picker.loading = None;
        if self.picker.open {
            match result {
                Ok(listing) => {
                    self.picker.directory = listing.directory;
                    self.picker.entries = listing.entries;
                    self.picker.truncated = listing.truncated;
                    self.picker.error = None;
                }
                Err(error) => self.picker.error = Some(error),
            }
        }
        if self.quitting && !self.busy() {
            self.close_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(key: &str, _: &[(&str, &str)]) -> String { key.into() }

    #[test]
    fn cancellation_and_stale_completion_do_not_select_an_image() {
        let mut e = Engine::new(label);
        e.picker.open = true;
        e.picker.loading = Some(7);
        e.picker_cancel();
        e.browsed(6, Ok(Listing::default()));
        assert_eq!(e.picker.loading, Some(7));
        e.browsed(7, Ok(Listing::default()));
        assert!(e.picker.loading.is_none());
        assert!(e.picker.selected.is_none());
        assert!(e.wallpaper.draft.is_none());
    }

    #[test]
    fn only_listed_files_can_be_selected() {
        let mut e = Engine::new(label);
        e.picker.open = true;
        e.picker.entries = vec![Entry { name:"image.png".into(), path:"/pictures/image.png".into(), directory:false }];
        e.picker_select("/pictures/elsewhere.png");
        assert!(e.picker.selected.is_none());
        e.picker_select("/pictures/image.png");
        assert!(e.can_picker_use());
    }
}
