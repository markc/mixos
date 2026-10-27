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
//! Re-pinned 2026-10-21 for the final modern-style source. A synthetic CBC2
//! comparison of 0ec2119 and feec5277 found that the default desktop's
//! resolved values are byte-identical except for the embedded source identity
//! in seven effective contexts and the snapshot header. The identity changes
//! because the embedded source now contains Breeze and Adwaita; the store
//! format and old selected/default values do not change. The bridge test below
//! keeps the previous pins active after replacing only those identity fields.
//!
//! Re-pinned 2026-10-23: the embedded source dropped 13 dead panel tokens
//! (clean break, theming amendment). Only the source identity moved: the
//! snapshot length is unchanged and the bridge test still reaches the
//! pre-modern pins once the identity fields are replaced.
//!
//! Re-pinned 2026-10-24 for Plasma-sized default UI and mono text. The prior
//! source fixture keeps the pre-modern byte bridge below meaningful: the new
//! source deliberately changes resolved typography as well as its identity.
//!
//! Re-pinned 2026-10-24 for the busviewer -> workshop clean rename. Only
//! the effective context key changes (one byte shorter); the source and
//! resolved values are unchanged. Restore that key as well as the prior
//! source identity/typography below to keep the older byte pins active.
use settings::*;

/// `settings::digest` of the default desktop's effective map.
const EFFECTIVE_DIGEST_PINNED: &str =
    "2021004311c4062a87733cd1c36119413f707591b4349bb6c671b2644a67efae";
/// `settings::digest` of the fixed default-desktop `Snapshot`.
const SNAPSHOT_DIGEST_PINNED: &str =
    "f8f5ad0ef813a69b9d08281d900a44f89683a364877ec3981b4d2bb77b0a862a";
/// Serialised byte length of that `Snapshot`.
const SNAPSHOT_JSON_LEN_PINNED: usize = 529_654;
const PRE_MODERN_SOURCE_DIGEST: &str =
    "b69369a6e36b10fd44a418df78cb1f2a99a40649e78027ce057af9993f648630";
const MODERN_SOURCE_DIGEST: &str =
    "74862423fc063d63e38cc20ed3264f9d9808dcf64f28b2073cfa73006ec32a4c";
const PRE_FONT_SOURCE_DIGEST: &str =
    "9ab8ab77799f670087a2f37c2ef0652d1795ef1a2ad25164212544d455ef07c7";
const PRE_MODERN_EFFECTIVE_DIGEST: &str =
    "7cd941ce5897d03a0ec10d4f2431214bfa7b3287fc1513d7d7c2c9bd09344147";
const PRE_MODERN_SNAPSHOT_DIGEST: &str =
    "5008077faa8a239b06904d9a85f2d1c7528a007df4a7b6ea0677e12a2a09c922";

fn fixed_snapshot() -> Snapshot {
    snapshot_with_source(EMBEDDED_DEFAULT_SOURCE)
}

fn snapshot_with_source(source: &str) -> Snapshot {
    let desktop = Desktop::default();
    let effective = resolve_with_embedded(&desktop, source).unwrap();
    Snapshot {
        schema: SCHEMA,
        binding: Binding {
            instance: "fixed".into(),
            profile: "default".into(),
        },
        incarnation: "00000000-0000-0000-0000-000000000001".into(),
        revision: Revision(1),
        design_revision: Revision(1),
        source_digest: source_digest(source),
        desktop,
        effective,
    }
}

#[test]
fn effective_and_snapshot_bytes_match_the_pinned_format() {
    let snapshot = fixed_snapshot();
    assert_eq!(snapshot.desktop.appearance.window_style, "mac");
    assert!(snapshot.effective.values().all(|e| e.window_style == "mac"));
    assert_eq!(
        (
            digest(&snapshot.effective).unwrap(),
            digest(&snapshot).unwrap(),
            serde_json::to_string(&snapshot).unwrap().len(),
            snapshot.source_digest.clone(),
        ),
        (
            EFFECTIVE_DIGEST_PINNED.to_owned(),
            SNAPSHOT_DIGEST_PINNED.to_owned(),
            SNAPSHOT_JSON_LEN_PINNED,
            MODERN_SOURCE_DIGEST.to_owned(),
        )
    );
}

#[test]
fn modern_embedded_source_changes_identity_but_no_default_resolved_value() {
    let mut snapshot =
        snapshot_with_source(include_str!("fixtures/2026-10-23-default.theme.conf.mix"));
    assert_eq!(snapshot.source_digest, PRE_FONT_SOURCE_DIGEST);
    for effective in snapshot.effective.values_mut() {
        assert_eq!(effective.design.source, PRE_FONT_SOURCE_DIGEST);
        effective.design.source = PRE_MODERN_SOURCE_DIGEST.into();
    }
    snapshot.source_digest = PRE_MODERN_SOURCE_DIGEST.into();
    restore_retired_context(&mut snapshot);
    assert_eq!(
        digest(&snapshot.effective).unwrap(),
        PRE_MODERN_EFFECTIVE_DIGEST
    );
    assert_eq!(digest(&snapshot).unwrap(), PRE_MODERN_SNAPSHOT_DIGEST);
}

fn restore_retired_context(snapshot: &mut Snapshot) {
    let effective = snapshot.effective.remove("app:workshop").unwrap();
    assert!(
        snapshot
            .effective
            .insert("app:busviewer".into(), effective)
            .is_none()
    );
}

#[test]
fn workshop_rename_changes_only_the_default_context_key() {
    let mut snapshot = fixed_snapshot();
    restore_retired_context(&mut snapshot);
    assert_eq!(
        digest(&snapshot.effective).unwrap(),
        "4abaa2fffc034923ac448fc8a9f8c93e253d2781be5a40b8a1b17f0f5e6422ea"
    );
    assert_eq!(
        digest(&snapshot).unwrap(),
        "08d7f6778302cbefc0993e3834678be5dbc465dc1a67d10ee29d8849292efe07"
    );
    assert_eq!(serde_json::to_string(&snapshot).unwrap().len(), 529_655);
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

#[test]
fn window_style_defaults_preserve_existing_fixture_seals() {
    for text in [
        include_str!("fixtures/before-axes/desktop.conf.mix"),
        include_str!("fixtures/before-axes/desktop.previous.conf.mix"),
        include_str!("fixtures/authored-legacy/desktop.conf.mix"),
        include_str!("../../../libs/design/tests/fixtures/sealed-profile.conf.mix"),
    ] {
        let mut accepted: settingsd::store::Accepted = strict::from_str(text).unwrap();
        assert_eq!(accepted.desktop.appearance.window_style, "mac");
        accepted.check(&accepted.binding).unwrap();
        let sealed = accepted.content_digest.clone();
        accepted.seal().unwrap();
        assert_eq!(accepted.content_digest, sealed);
        let saved = serde_json::to_value(&accepted).unwrap();
        assert!(
            saved["desktop"]["appearance"]
                .get("window_style")
                .is_none()
        );
    }
}
