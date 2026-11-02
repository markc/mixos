// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Reading a Wallpaper folder off the window thread: the images under it,
//! bounded in depth and count (spec 1 §4.1: depth 3, at most 500).
use preferences::gallery::{MAX_DEPTH, MAX_IMAGES};
use std::path::{Path, PathBuf};

/// The extensions a wallpaper may have (the registry's image paths).
const EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
const MAX_DIRECTORIES: usize = 128;
const MAX_ENTRIES: usize = 2048;

pub struct Scan {
    pub images: Vec<String>,
    pub truncated: bool,
}

fn image(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| EXTENSIONS.iter().any(|e| s.eq_ignore_ascii_case(e)))
}

/// The images under `folder`, at most [`MAX_DEPTH`] folders down and
/// [`MAX_IMAGES`] in all, by path. Hidden entries are skipped.
pub fn images(folder: &str) -> Result<Scan, String> {
    let root = PathBuf::from(folder);
    if !root.is_absolute() {
        return Err(crate::label("wallpaper-folder-invalid"));
    }
    std::fs::read_dir(&root).map_err(|e| e.to_string())?;
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    let mut truncated = false;
    let mut queued = 1;
    let mut folders = vec![(root, 0)];
    'walk: while let Some((dir, depth)) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for (index, entry) in entries.enumerate() {
            if index == MAX_ENTRIES {
                truncated = true;
                break;
            }
            let Ok(entry) = entry else { continue };
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if depth + 1 < MAX_DEPTH {
                    if queued < MAX_DIRECTORIES {
                        queued += 1;
                        folders.push((path, depth + 1));
                    } else {
                        truncated = true;
                    }
                }
            } else if (kind.is_file()
                || (kind.is_symlink()
                    && path.canonicalize().is_ok_and(|p| p.starts_with(&canonical_root) && p.is_file())))
                && image(&path)
                && let Some(text) = path.to_str()
            {
                found.push(text.to_owned());
                if found.len() == MAX_IMAGES {
                    truncated = true;
                    break 'walk;
                }
            }
        }
    }
    found.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
    Ok(Scan { images: found, truncated })
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
                .images
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
        assert!(images(dir.0.to_str().unwrap()).unwrap().truncated);
        assert!(images(dir.0.join("missing").to_str().unwrap()).is_err());
        assert!(images("relative").is_err());
    }

    #[test]
    fn directory_and_entry_budgets_are_reported() {
        let dir = Directory::new();
        for i in 0..MAX_DIRECTORIES + 1 {
            std::fs::create_dir(dir.0.join(format!("d{i}"))).unwrap();
        }
        assert!(images(dir.0.to_str().unwrap()).unwrap().truncated);
        let dir = Directory::new();
        for i in 0..MAX_ENTRIES + 1 {
            std::fs::write(dir.0.join(format!("n{i}.txt")), []).unwrap();
        }
        assert!(images(dir.0.to_str().unwrap()).unwrap().truncated);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_directories_and_external_files_are_excluded() {
        use std::os::unix::fs::symlink;
        let dir = Directory::new();
        let outside = Directory::new();
        std::fs::write(outside.0.join("external.png"), []).unwrap();
        std::fs::write(dir.0.join("inside.png"), []).unwrap();
        symlink(&outside.0, dir.0.join("escape")).unwrap();
        symlink(&dir.0, dir.0.join("loop")).unwrap();
        symlink(outside.0.join("external.png"), dir.0.join("external.png")).unwrap();
        symlink(dir.0.join("inside.png"), dir.0.join("alias.png")).unwrap();
        assert_eq!(dir.names(), ["/alias.png", "/inside.png"]);
    }
}
