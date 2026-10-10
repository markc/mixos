// SPDX-License-Identifier: MIT OR Apache-2.0
//! The sealed format is untouched by the in-memory accent sidecar. The digests
//! below were taken from the tree before the sidecar existed (27efdba, probed on
//! the CBC worker for the default desktop). A change to `Snapshot` or `Effective`
//! that alters their serialised bytes changes these digests and fails here.
use settings::*;

/// `settings::digest` of the default desktop's effective map, pre-sidecar.
const EFFECTIVE_DIGEST_BEFORE_SIDECAR: &str =
    "4c25711ceab88f6d50bf60c5df943a6f8c5001189927987bb504dc0c809b4ab2";
/// `settings::digest` of the fixed default-desktop `Snapshot`, pre-sidecar.
const SNAPSHOT_DIGEST_BEFORE_SIDECAR: &str =
    "88d1ace5ea6370d37120cf8450ad10b288f68532dd695b0d1a0be4cb69a498c7";
/// Serialised byte length of that `Snapshot`, pre-sidecar.
const SNAPSHOT_JSON_LEN_BEFORE_SIDECAR: usize = 529_312;

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
fn effective_and_snapshot_bytes_match_the_pre_sidecar_format() {
    let snapshot = fixed_snapshot();
    assert_eq!(
        digest(&snapshot.effective).unwrap(),
        EFFECTIVE_DIGEST_BEFORE_SIDECAR
    );
    assert_eq!(digest(&snapshot).unwrap(), SNAPSHOT_DIGEST_BEFORE_SIDECAR);
    assert_eq!(
        serde_json::to_string(&snapshot).unwrap().len(),
        SNAPSHOT_JSON_LEN_BEFORE_SIDECAR
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
