// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The manual built into the binary: `docs/mix/*.md` as one zstd frame,
//! packed by `build.rs`. It is the copy that matches this binary, so `mix man`
//! reads it first and a binary copied alone (no checkout, no network) still
//! has its manual. Decompressed once, on first use.
use std::io::Read;
use std::sync::OnceLock;

static PACKED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/manual.zst"));

/// The compressed size, for `mix doctor`.
pub const COMPRESSED_BYTES: usize = PACKED.len();

/// `(filename, body)` per page, in filename order.
fn pages() -> &'static [(String, String)] {
    static PAGES: OnceLock<Vec<(String, String)>> = OnceLock::new();
    PAGES.get_or_init(|| unpack(PACKED).unwrap_or_default())
}

fn unpack(compressed: &[u8]) -> Option<Vec<(String, String)>> {
    let mut source = compressed;
    let mut decoder = structured_zstd::decoding::StreamingDecoder::new(&mut source).ok()?;
    let mut packed = Vec::new();
    decoder.read_to_end(&mut packed).ok()?;
    fn take(rest: &mut &[u8]) -> Option<String> {
        let s: &[u8] = *rest;
        let len = u32::from_le_bytes(s.get(..4)?.try_into().ok()?) as usize;
        let field = s.get(4..4 + len)?;
        *rest = &s[4 + len..];
        String::from_utf8(field.to_vec()).ok()
    }
    let mut rest = packed.as_slice();
    let mut pages = Vec::new();
    while !rest.is_empty() {
        let name = take(&mut rest)?;
        let body = take(&mut rest)?;
        pages.push((name, body));
    }
    Some(pages)
}

/// The embedded page for a `topic.md` filename (`README.md` is the index).
pub fn page(filename: &str) -> Option<&'static str> {
    pages()
        .iter()
        .find(|(name, _)| name == filename)
        .map(|(_, body)| body.as_str())
}

/// Embedded topic names, sorted, without the index.
pub fn topics() -> Vec<String> {
    pages()
        .iter()
        .filter(|(name, _)| name != "README.md")
        .map(|(name, _)| name.trim_end_matches(".md").to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn every_checkout_page_is_embedded_byte_for_byte() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/mix");
        let mut on_disk = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            if !name.ends_with(".md") {
                continue;
            }
            on_disk += 1;
            let body = std::fs::read_to_string(dir.join(&name)).unwrap();
            assert_eq!(page(&name), Some(body.as_str()), "{name}");
        }
        assert_eq!(pages().len(), on_disk);
        assert!(page("overview.md").is_some() && page("README.md").is_some());
        assert!(!topics().contains(&"README".to_string()));
    }

    #[test]
    fn the_manual_is_stored_compressed() {
        let raw: usize = pages().iter().map(|(n, b)| n.len() + b.len() + 8).sum();
        assert!(
            COMPRESSED_BYTES * 3 < raw,
            "{COMPRESSED_BYTES} compressed of {raw}"
        );
    }

    #[test]
    fn a_damaged_pack_is_no_manual_not_a_panic() {
        assert!(unpack(b"not zstd").is_none());
        let truncated = &PACKED[..PACKED.len() / 2];
        assert!(unpack(truncated).is_none_or(|pages| pages.len() < topics().len()));
    }
}
