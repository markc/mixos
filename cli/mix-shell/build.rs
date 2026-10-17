// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use std::path::{Path, PathBuf};

fn main() {
    buildinfo::emit();
    pack_manual();
}

/// Pack `docs/mix/*.md` into one zstd frame for `src/manual.rs`, so a binary
/// copied alone still has `mix man`. The layout is, per page in name order,
/// `u32 LE name length, name, u32 LE body length, body`.
fn pack_manual() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../docs/mix");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".md"))
        .collect();
    names.sort();
    let mut packed = Vec::new();
    for name in &names {
        let path = dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        let body = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        packed.extend_from_slice(&(name.len() as u32).to_le_bytes());
        packed.extend_from_slice(name.as_bytes());
        packed.extend_from_slice(&(body.len() as u32).to_le_bytes());
        packed.extend_from_slice(&body);
    }
    let compressed = structured_zstd::encoding::compress_slice_to_vec(
        &packed,
        structured_zstd::encoding::CompressionLevel::Best,
    );
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("manual.zst");
    std::fs::write(&out, compressed).unwrap_or_else(|e| panic!("write {}: {e}", out.display()));
}
