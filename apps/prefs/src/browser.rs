// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded image-directory I/O, run off the window thread.
use preferences::picker::{Entry, Listing, MAX_ENTRIES};
use std::path::{Path, PathBuf};

pub fn list(path: Option<String>) -> Result<Listing, String> {
    let root = match path {
        Some(path) => PathBuf::from(path),
        None => starting_directory(),
    };
    if !root.is_absolute() {
        return Err(crate::label("wallpaper-path-invalid"));
    }
    let directory = root.to_str().ok_or_else(|| crate::label("wallpaper-path-invalid"))?.to_owned();
    let iter = std::fs::read_dir(&root).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for (index, entry) in iter.enumerate() {
        if index == MAX_ENTRIES {
            truncated = true;
            break;
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let Some(path_text) = path.to_str() else { continue; };
        let Ok(metadata) = std::fs::metadata(&path) else { continue; };
        let directory = metadata.is_dir();
        let image = metadata.is_file() && path.extension().and_then(|s| s.to_str())
            .is_some_and(|s| ["png", "jpg", "jpeg"].iter().any(|extension| s.eq_ignore_ascii_case(extension)));
        if directory || image {
            entries.push(Entry { name:entry.file_name().to_string_lossy().into_owned(), path:path_text.into(), directory });
        }
    }
    entries.sort_by(|a,b| b.directory.cmp(&a.directory).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name)));
    Ok(Listing { directory, entries, truncated })
}

fn starting_directory() -> PathBuf {
    let wallpapers = Path::new("/opt/mixos/share/wallpapers");
    if wallpapers.is_dir() { return wallpapers.into(); }
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from("/"));
    let pictures = home.join("Pictures");
    if pictures.is_dir() { pictures } else { home }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct Directory(PathBuf);
    impl Directory {
        fn new()->Self {
            static NEXT:AtomicU64=AtomicU64::new(0);
            let path=std::env::temp_dir().join(format!("prefs-browser-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
            std::fs::create_dir(&path).unwrap(); Self(path)
        }
        fn listing(&self)->Listing {list(Some(self.0.to_str().unwrap().into())).unwrap()}
    }
    impl Drop for Directory {fn drop(&mut self){std::fs::remove_dir_all(&self.0).unwrap();}}
    #[test]
    fn browser_lists_directories_and_image_extensions_in_stable_order() {
        let dir=Directory::new();
        std::fs::create_dir(dir.0.join("folder")).unwrap();
        for name in ["B.JPG","a.png","c.Jpeg","hidden.txt"] {std::fs::write(dir.0.join(name),[]).unwrap();}
        let listing=dir.listing();
        assert_eq!(listing.entries.iter().map(|e|e.name.as_str()).collect::<Vec<_>>(),["folder","a.png","B.JPG","c.Jpeg"]);
        assert!(listing.entries[0].directory); assert!(!listing.truncated);
    }
    #[test]
    fn directory_scan_has_a_hard_entry_limit_and_reports_errors() {
        let dir=Directory::new();
        for index in 0..MAX_ENTRIES+1 {std::fs::write(dir.0.join(format!("{index}.png")),[]).unwrap();}
        let listing=dir.listing(); assert!(listing.truncated); assert_eq!(listing.entries.len(),MAX_ENTRIES);
        assert!(list(Some(dir.0.join("missing").to_str().unwrap().into())).is_err());
        assert!(list(Some("relative".into())).is_err());
    }
}
