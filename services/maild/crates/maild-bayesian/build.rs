// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

// Emit MIXOS_GIT_SHA / MIXOS_GIT_DIRTY / MIXOS_BUILD_TIME so `--version`
// reports this crate's build provenance (buildinfo::exit_on_version!).
fn main() {
    buildinfo::emit();
}
