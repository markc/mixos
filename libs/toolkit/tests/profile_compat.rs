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

#[test]
fn a_copied_sealed_profile_keeps_its_format_and_digests() {
    let source = include_str!("../../design/tests/fixtures/sealed-profile.conf.mix");
    let mut record: store::Accepted = strict::from_str(source).unwrap();
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
    assert_eq!(record.format, store::FORMAT);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("desktop.conf.mix");
    std::fs::write(&path, source).unwrap();
    let before = std::fs::read(&path).unwrap();
    let (_, opened) = store::Store::open(root.path(), &record.binding).unwrap();
    assert_eq!(opened.format, record.format);
    assert_eq!(opened.content_digest, record.content_digest);
    assert_eq!(opened.effective_digest, record.effective_digest);
    assert_eq!(std::fs::read(path).unwrap(), before);
    let theme = toolkit::Theme::from_source_in(
        "sealed-profile",
        &record.embedded_source,
        design::DesignContext::default(),
    )
    .unwrap();
    assert!(!theme.dictionary().defaulted.is_empty());
}
