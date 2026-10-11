// SPDX-License-Identifier: MIT OR Apache-2.0
//! The icon for a file by its kind: folders open or shut, then a few
//! families by extension.

use std::path::Path;

use toolkit::Icon;

/// The icon for `path`: a folder (open when expanded), else by extension.
pub fn file_icon(path: &Path, is_dir: bool, expanded: bool) -> Icon {
    if is_dir {
        return if expanded {
            Icon::FolderOpen
        } else {
            Icon::Folder
        };
    }
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mid" | "midi" | "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a" | "aac") => {
            Icon::FileMusic
        }
        Some("mp4" | "mkv" | "webm" | "mov" | "avi" | "mpeg" | "mpg") => Icon::FileVideoCamera,
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tiff") => Icon::FileImage,
        Some("rs" | "c" | "h" | "cpp" | "js" | "ts" | "html" | "css" | "sh" | "mix") => {
            Icon::FileCode
        }
        Some("zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar") => Icon::FileArchive,
        Some("txt" | "md" | "pdf" | "doc" | "docx" | "odt" | "csv" | "toml" | "json") => {
            Icon::FileText
        }
        _ => Icon::File,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_and_families() {
        let icon = |p: &str, dir, open| file_icon(Path::new(p), dir, open);
        assert_eq!(icon("/x", true, false), Icon::Folder);
        assert_eq!(icon("/x", true, true), Icon::FolderOpen);
        assert_eq!(icon("a.FLAC", false, false), Icon::FileMusic);
        assert_eq!(icon("a.mkv", false, false), Icon::FileVideoCamera);
        assert_eq!(icon("a.rs", false, false), Icon::FileCode);
        assert_eq!(icon("a.tar", false, false), Icon::FileArchive);
        assert_eq!(icon("a.md", false, false), Icon::FileText);
        assert_eq!(icon("a.bin", false, false), Icon::File);
        assert_eq!(icon("noext", false, false), Icon::File);
    }
}
