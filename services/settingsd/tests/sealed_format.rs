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
use settings::*;

/// `settings::digest` of the default desktop's effective map.
const EFFECTIVE_DIGEST_PINNED: &str =
    "9c04e43bcff473f26c41ebc3f8a3286e0322ca67139f5d5e1a032cf768a48a29";
/// `settings::digest` of the fixed default-desktop `Snapshot`.
const SNAPSHOT_DIGEST_PINNED: &str =
    "21c07dbaa05f3ecafec3ce0ee276f87e4292869513df4a9b81b787d5a9b632f5";
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
    assert_eq!(digest(&snapshot.effective).unwrap(), EFFECTIVE_DIGEST_PINNED);
    assert_eq!(digest(&snapshot).unwrap(), SNAPSHOT_DIGEST_PINNED);
    assert_eq!(
        serde_json::to_string(&snapshot).unwrap().len(),
        SNAPSHOT_JSON_LEN_PINNED
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
