// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Exercise the owning store implementation without rebuilding settingsd.

#[allow(
    dead_code,
    reason = "the test imports the complete owning store module"
)]
#[path = "../../../services/settingsd/src/store.rs"]
mod store;

#[allow(
    dead_code,
    reason = "the owning store calls its native snapshot validator"
)]
#[path = "../../../services/settingsd/src/authority.rs"]
mod authority;

#[allow(dead_code, reason = "the owning store reads format-4 records")]
#[path = "../../../services/settingsd/src/format4.rs"]
mod format4;

#[allow(dead_code, reason = "the owning store migrates format 4 on open")]
#[path = "../../../services/settingsd/src/migration.rs"]
mod migration;

#[allow(dead_code, reason = "the owning store keeps the mirror's cache aside")]
#[path = "../../../services/settingsd/src/mirror.rs"]
mod mirror;

#[test]
fn a_copied_sealed_profile_keeps_its_format_and_digests() {
    let source = include_str!("../../design/tests/fixtures/sealed-profile.conf.mix");
    let mut record: format4::Record = strict::from_str(source).unwrap();
    if std::env::var_os("UPDATE_PROFILE_FIXTURE").is_some() {
        record.effective_digest = settings::digest(
            &settings::resolve_with_embedded(&record.desktop, &record.embedded_source).unwrap(),
        )
        .unwrap();
        record.seal().unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../design/tests/fixtures/sealed-profile.conf.mix");
        std::fs::write(
            path,
            format!(
                "# SPDX-License-Identifier: MIT OR Apache-2.0\n{}\n",
                serde_json::to_string_pretty(&record).unwrap()
            ),
        )
        .unwrap();
        return;
    }
    assert_eq!(record.format, format4::FORMAT);
    record.check(&record.binding).unwrap();
    record.effective().unwrap();
    // The store migrates it on open, keeping the format-4 record as it was.
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("desktop.conf.mix"), source).unwrap();
    let (_, opened) = store::Store::open(root.path(), &record.binding).unwrap();
    assert_eq!(opened.format, store::FORMAT);
    assert_eq!(opened.incarnation, record.incarnation);
    opened.effective().unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join(migration::KEPT)).unwrap(),
        source
    );
    let theme = toolkit::Theme::from_source_in(
        "sealed-profile",
        &record.embedded_source,
        design::DesignContext::default(),
    )
    .unwrap();
    assert!(!theme.dictionary().defaulted.is_empty());
}
