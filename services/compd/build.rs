// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Embed build provenance (git sha, dirty bit, build time, and the
// MIXOS-BUILDINFO:1 marker) for `compd --version`.
fn main() {
    buildinfo::emit();
}
