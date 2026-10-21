// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The sealed format is untouched by the in-memory accent sidecar. The digests
//! below pin the serialised bytes of the default desktop's `Snapshot` and
//! `Effective`. A change to `Snapshot` or `Effective` that alters those bytes
//! changes these digests and fails here, so a re-pin must be deliberate.
//!
//! Re-pinned 2026-10-18: the default design gained the token
//! `icon.stroke_width` (1.5, commit 93dfa3fa, "design: author a 1.5 Lucide
//! stroke baseline"). That token is part of the default effective map, so the
//! pinned bytes moved. The store format is unchanged; only the default design
//! changed.
//!
//! Re-pinned 2026-10-21 for the accepted phase-one theming defaults. The
//! previous pins pass on the Markdown parent and fail on the untouched
//! theming parent. Only default-design values changed: the JSON byte length
//! and store format stay unchanged, with old sealed profiles covered by the
//! compatibility and transaction tests.
use settings::*;

/// `settings::digest` of the default desktop's effective map.
const EFFECTIVE_DIGEST_PINNED: &str =
    "7cd941ce5897d03a0ec10d4f2431214bfa7b3287fc1513d7d7c2c9bd09344147";
/// `settings::digest` of the fixed default-desktop `Snapshot`.
const SNAPSHOT_DIGEST_PINNED: &str =
    "5008077faa8a239b06904d9a85f2d1c7528a007df4a7b6ea0677e12a2a09c922";
/// Serialised byte length of that `Snapshot`.
const SNAPSHOT_JSON_LEN_PINNED: usize = 529_634;

fn fixed_snapshot() -> Snapshot {
    let desktop = Desktop::default();
    let effective = resolve(&desktop).unwrap();
    Snapshot {
        schema: SCHEMA,
        binding: Binding {
            instance: "fixed".into(),
            profile: "default".into(),
        },
        incarnation: "00000000-0000-0000-0000-000000000001".into(),
        revision: Revision(1),
        design_revision: Revision(1),
        source_digest: source_digest(EMBEDDED_DEFAULT_SOURCE),
        desktop,
        effective,
    }
}

#[test]
fn effective_and_snapshot_bytes_match_the_pinned_format() {
    let snapshot = fixed_snapshot();
    assert_eq!(
        (
            digest(&snapshot.effective).unwrap(),
            digest(&snapshot).unwrap(),
            serde_json::to_string(&snapshot).unwrap().len(),
        ),
        (
            EFFECTIVE_DIGEST_PINNED.to_owned(),
            SNAPSHOT_DIGEST_PINNED.to_owned(),
            SNAPSHOT_JSON_LEN_PINNED,
        )
    );
}

#[test]
fn resolve_and_the_accent_variant_agree_on_the_effective_map() {
    let desktop = Desktop::default();
    let plain = resolve(&desktop).unwrap();
    let (with_accents, accents) =
        resolve_with_embedded_and_accents(&desktop, EMBEDDED_DEFAULT_SOURCE).unwrap();
    assert_eq!(plain, with_accents);
    assert!(accents.contains_key("desktop"));
}
