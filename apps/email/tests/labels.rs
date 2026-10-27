// SPDX-License-Identifier: MIT OR Apache-2.0
//! Literal label keys in the application must exist in the English catalogue.
use std::path::Path;

fn check_source(path: &Path, checked: &mut usize) {
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            check_source(&path, checked);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).unwrap();
            for call in ["label(", "label_with("] {
                for (start, _) in source.match_indices(call) {
                    // Exclude egui's .label, get_by_label and Registry::relabel.
                    if source[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '.')) { continue; }
                    let tail = &source[start + call.len()..];
                    let Some(literal) = tail.trim_start().strip_prefix('"') else { continue };
                    let key = literal.split('"').next().unwrap();
                    assert!(email::CATALOGUE.lines().any(|line| line.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='))),
                        "{}: missing label {key}", path.display());
                    *checked += 1;
                }
            }
        }
    }
}

#[test]
fn every_literal_label_key_in_email_source_resolves() {
    // Loading also rejects duplicate keys and malformed Fluent messages.
    let _ = email::strings();
    let mut checked = 0;
    check_source(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut checked);
    assert!(checked > 100, "the source scanner must visit the app");
}
