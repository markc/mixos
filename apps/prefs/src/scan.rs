// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Reading a Wallpaper folder off the window thread: the images under it,
//! bounded in depth and count (spec 1 §4.1: depth 3, at most 500).
use preferences::gallery::{MAX_DEPTH, MAX_IMAGES};
use std::path::{Path, PathBuf};

/// The extensions a wallpaper may have (the registry's image paths).
const EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

fn image(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| EXTENSIONS.iter().any(|e| s.eq_ignore_ascii_case(e)))
}

/// The images under `folder`, at most [`MAX_DEPTH`] folders down and
/// [`MAX_IMAGES`] in all, by path. Hidden entries are skipped.
pub fn images(folder: &str) -> Result<Vec<String>, String> {
    let root = PathBuf::from(folder);
    if !root.is_absolute() {
        return Err(crate::label("wallpaper-folder-invalid"));
    }
    std::fs::read_dir(&root).map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    let mut folders = vec![(root, 0)];
    while let Some((dir, depth)) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                if depth + 1 < MAX_DEPTH {
                    folders.push((path, depth + 1));
                }
            } else if metadata.is_file()
                && image(&path)
                && let Some(text) = path.to_str()
            {
                found.push(text.to_owned());
            }
        }
    }
    found.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
    found.truncate(MAX_IMAGES);
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "prefs-scan-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn names(&self) -> Vec<String> {
            let root = self.0.to_str().unwrap();
            images(root)
                .unwrap()
                .iter()
                .map(|p| p.strip_prefix(root).unwrap().to_owned())
                .collect()
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn images_three_folders_deep_in_stable_order() {
        let dir = Directory::new();
        std::fs::create_dir_all(dir.0.join("a/b/c")).unwrap();
        for name in ["B.JPG", "a.png", "c.webp", "notes.txt", ".hidden.png"] {
            std::fs::write(dir.0.join(name), []).unwrap();
        }
        std::fs::write(dir.0.join("a/b/deep.png"), []).unwrap();
        std::fs::write(dir.0.join("a/b/c/too-deep.png"), []).unwrap();
        assert_eq!(
            dir.names(),
            ["/a.png", "/a/b/deep.png", "/B.JPG", "/c.webp"]
        );
    }

    #[test]
    fn a_folder_has_a_hard_image_limit_and_reports_errors() {
        let dir = Directory::new();
        for index in 0..MAX_IMAGES + 1 {
            std::fs::write(dir.0.join(format!("{index}.png")), []).unwrap();
        }
        assert_eq!(dir.names().len(), MAX_IMAGES);
        assert!(images(dir.0.join("missing").to_str().unwrap()).is_err());
        assert!(images("relative").is_err());
    }
}
