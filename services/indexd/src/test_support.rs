// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Ported from cosmix-indexd/src/main.rs.

use std::path::PathBuf;
pub(crate) fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("indexd-cfg-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
