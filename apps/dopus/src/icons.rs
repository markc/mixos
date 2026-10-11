// SPDX-License-Identifier: MIT OR Apache-2.0
//! The icon for a place, a folder or a file.
//!
//! - Places and the user's well-known folders (Documents, Downloads, Music,
//!   Pictures, Videos, Desktop, Templates, Public) show what they hold.
//! - Files show their kind by MIME type, looked up from the extension in
//!   shared-mime-info's `globs2` (the highest-weighted glob wins), then
//!   grouped: text, code, image, audio, video, archive, spreadsheet,
//!   presentation, font, disk image, executable. Without `globs2` a short
//!   built-in table stands in.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use toolkit::Icon;

/// shared-mime-info's glob table.
const GLOBS: &str = "/usr/share/mime/globs2";

/// The icon for a place by its name (the core's Places), else a folder.
pub fn place_icon(name: &str) -> Icon {
    match name {
        "Home" => Icon::House,
        "Filesystem" => Icon::HardDrive,
        other => known_folder(other).unwrap_or(Icon::Folder),
    }
}

/// The icon for one of the user's well-known folders, by its name.
fn known_folder(name: &str) -> Option<Icon> {
    Some(match name {
        "Desktop" => Icon::Monitor,
        "Documents" => Icon::FileText,
        "Downloads" => Icon::Download,
        "Music" => Icon::Music,
        "Pictures" => Icon::Image,
        "Videos" => Icon::Film,
        "Templates" => Icon::LayoutTemplate,
        "Public" => Icon::Globe,
        _ => return None,
    })
}

/// The icon for `path`. A folder is open when expanded; a well-known
/// folder directly in the home directory shows what it holds.
pub fn file_icon(path: &Path, is_dir: bool, expanded: bool) -> Icon {
    if is_dir {
        let home = dopus_core::home_directory();
        let known = (path.parent() == Some(home.as_path()))
            .then(|| path.file_name().and_then(|n| n.to_str()).and_then(known_folder))
            .flatten();
        return match (known, expanded) {
            (Some(icon), _) => icon,
            (None, true) => Icon::FolderOpen,
            (None, false) => Icon::Folder,
        };
    }
    let Some(ext) = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return Icon::File;
    };
    match mime_of(&ext) {
        Some(mime) => mime_icon(mime),
        None => fallback(&ext),
    }
}

/// The MIME type for an extension, from shared-mime-info.
fn mime_of(ext: &str) -> Option<&'static str> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            std::fs::read_to_string(GLOBS)
                .map(|text| parse_globs(&text))
                .unwrap_or_default()
        })
        .get(ext)
        .map(String::as_str)
}

/// `weight:mime:*.ext[:flags]` lines to an extension → MIME map. The file
/// lists higher weights first, so the first entry for an extension wins.
pub fn parse_globs(text: &str) -> HashMap<String, String> {
    let mut table = HashMap::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let mut parts = line.split(':');
        let (Some(_weight), Some(mime), Some(glob)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Some(ext) = glob.strip_prefix("*.") else {
            continue;
        };
        if ext.contains(['*', '?', '[']) {
            continue;
        }
        table
            .entry(ext.to_ascii_lowercase())
            .or_insert_with(|| mime.to_owned());
    }
    table
}

/// The icon for a MIME type.
pub fn mime_icon(mime: &str) -> Icon {
    let (top, sub) = mime.split_once('/').unwrap_or((mime, ""));
    let has = |words: &[&str]| words.iter().any(|w| sub.contains(w));
    match top {
        "image" => Icon::FileImage,
        "audio" => Icon::FileMusic,
        "video" => Icon::FileVideoCamera,
        "font" => Icon::FileType,
        _ if has(&["spreadsheet", "excel", "csv"]) => Icon::FileSpreadsheet,
        _ if has(&["presentation", "powerpoint"]) => Icon::Presentation,
        _ if has(&[
            "zip", "tar", "gzip", "bzip", "xz", "7z", "rar", "zstd", "lzma", "lz4", "compress",
            "archive", "cpio", "rpm", "debian-binary",
        ]) =>
        {
            Icon::FileArchive
        }
        _ if has(&["iso", "disk-image", "raw-disk", "cd-image", "rom"]) => Icon::Disc,
        _ if has(&["executable", "sharedlib", "appimage", "elf", "object"]) => Icon::FileCog,
        _ if has(&["shellscript"]) => Icon::FileTerminal,
        "text" if has(&["markdown", "plain", "rtf"]) => Icon::FileText,
        "text" => Icon::FileCode,
        _ if has(&[
            "json", "xml", "javascript", "toml", "yaml", "sql", "x-python", "x-perl", "x-php",
            "x-ruby", "wasm",
        ]) =>
        {
            Icon::FileCode
        }
        _ if has(&["pdf", "document", "word", "opendocument.text", "epub", "postscript"]) => {
            Icon::FileText
        }
        _ => Icon::File,
    }
}

/// Without shared-mime-info: the common families by extension.
fn fallback(ext: &str) -> Icon {
    match ext {
        "mid" | "midi" | "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a" | "aac" => Icon::FileMusic,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "mpeg" | "mpg" => Icon::FileVideoCamera,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tiff" => Icon::FileImage,
        "rs" | "c" | "h" | "cpp" | "js" | "ts" | "html" | "css" | "mix" | "json" | "toml" => {
            Icon::FileCode
        }
        "sh" => Icon::FileTerminal,
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "zst" => Icon::FileArchive,
        "txt" | "md" | "pdf" | "doc" | "docx" | "odt" => Icon::FileText,
        "csv" | "ods" | "xlsx" | "xls" => Icon::FileSpreadsheet,
        "iso" | "img" => Icon::Disc,
        _ => Icon::File,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_and_known_folders_show_what_they_hold() {
        assert_eq!(place_icon("Home"), Icon::House);
        assert_eq!(place_icon("Documents"), Icon::FileText);
        assert_eq!(place_icon("Downloads"), Icon::Download);
        assert_eq!(place_icon("Pictures"), Icon::Image);
        assert_eq!(place_icon("Videos"), Icon::Film);
        assert_eq!(place_icon("Elsewhere"), Icon::Folder);
        let home = dopus_core::home_directory();
        assert_eq!(file_icon(&home.join("Music"), true, false), Icon::Music);
        assert_eq!(file_icon(&home.join("x/Music"), true, false), Icon::Folder);
        assert_eq!(file_icon(Path::new("/x"), true, true), Icon::FolderOpen);
    }

    #[test]
    fn globs_take_the_first_and_heaviest_entry() {
        let text = "# comment\n80:text/html:*.html\n50:application/vnd.efi.iso:*.iso\n\
                    50:application/x-wii-rom:*.iso\n50:text/x-c:*.C:cs\n10:text/x-readme:README*\n";
        let table = parse_globs(text);
        assert_eq!(table["iso"], "application/vnd.efi.iso");
        assert_eq!(table["html"], "text/html");
        assert_eq!(table["c"], "text/x-c");
        assert!(!table.contains_key("readme*"));
    }

    #[test]
    fn mime_types_group_into_families() {
        for (mime, icon) in [
            ("image/png", Icon::FileImage),
            ("audio/flac", Icon::FileMusic),
            ("video/mp4", Icon::FileVideoCamera),
            ("font/ttf", Icon::FileType),
            ("text/markdown", Icon::FileText),
            ("text/rust", Icon::FileCode),
            ("text/csv", Icon::FileSpreadsheet),
            ("application/pdf", Icon::FileText),
            ("application/json", Icon::FileCode),
            ("application/toml", Icon::FileCode),
            ("application/zip", Icon::FileArchive),
            ("application/x-bzip2", Icon::FileArchive),
            ("application/zstd", Icon::FileArchive),
            ("application/vnd.debian.binary-package", Icon::FileArchive),
            ("application/vnd.oasis.opendocument.spreadsheet", Icon::FileSpreadsheet),
            ("application/vnd.oasis.opendocument.presentation", Icon::Presentation),
            ("application/vnd.oasis.opendocument.text", Icon::FileText),
            ("application/vnd.efi.iso", Icon::Disc),
            ("application/x-executable", Icon::FileCog),
            ("application/x-shellscript", Icon::FileTerminal),
            ("text/x-shellscript", Icon::FileTerminal),
            ("application/octet-stream", Icon::File),
        ] {
            assert_eq!(mime_icon(mime), icon, "{mime}");
        }
        assert_eq!(fallback("tar"), Icon::FileArchive);
        assert_eq!(file_icon(Path::new("noext"), false, false), Icon::File);
    }
}
