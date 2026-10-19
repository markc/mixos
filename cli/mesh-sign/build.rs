// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Emit build provenance so `--version` reports this crate's build
// (buildinfo::exit_on_version!).
fn main() {
    buildinfo::emit();
}
