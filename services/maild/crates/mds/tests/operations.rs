// SPDX-License-Identifier: MIT OR Apache-2.0
//! Storage checks formerly exercised through the operator CLI. Argument parsing,
//! environment root selection, exit codes and output formatting belonged to that
//! removed binary; these tests call the unchanged library contract directly.

use mds::{ContainerAttrs, Error, Flags, Mds, Membership, SetId, SqliteCasMds, VerifyScope};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

fn attrs() -> ContainerAttrs {
    ContainerAttrs {
        special_use: None,
        subscribed: true,
        extra: serde_json::json!({}),
    }
}

fn populated() -> (TempDir, SqliteCasMds, SetId, mds::ContainerId) {
    let root = TempDir::new().unwrap();
    let store = SqliteCasMds::open(root.path()).unwrap();
    let set = SetId(uuid::Uuid::now_v7());
    store.create_set(&set).unwrap();
    let inbox = store
        .create_container(&set, None, "INBOX", attrs())
        .unwrap();
    let blob = store.put_blob(b"hello").unwrap();
    store
        .add_item(
            &set,
            &blob,
            &[Membership {
                container: inbox,
                flags: Flags(0),
                added_at: 1,
            }],
        )
        .unwrap();
    (root, store, set, inbox)
}

#[test]
fn explicit_root_is_honoured() {
    let root = TempDir::new().unwrap();
    let store = SqliteCasMds::open(root.path()).unwrap();
    assert!(store.list_sets().unwrap().is_empty());
    assert!(root.path().join("blobs.sqlite").is_file());
}

#[test]
fn list_sets_returns_only_set_ids() {
    let (_root, store, first, _) = populated();
    let second = SetId(uuid::Uuid::now_v7());
    store.create_set(&second).unwrap();
    let sets = store.list_sets().unwrap();
    assert_eq!(sets.len(), 2);
    assert!(sets.contains(&first));
    assert!(sets.contains(&second));
}

#[test]
fn list_sets_ids_round_trip_via_serde() {
    let (_root, store, set, _) = populated();
    let json = serde_json::to_string(&store.list_sets().unwrap()).unwrap();
    let sets: Vec<SetId> = serde_json::from_str(&json).unwrap();
    assert_eq!(sets, vec![set]);
}

#[test]
fn migrate_all_on_empty_root_opens_zero_sets() {
    let root = TempDir::new().unwrap();
    assert!(
        SqliteCasMds::open(root.path())
            .unwrap()
            .list_sets()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn migrate_all_opens_every_existing_set() {
    let root = TempDir::new().unwrap();
    let store = SqliteCasMds::open(root.path()).unwrap();
    let sets: Vec<_> = (0..3).map(|_| SetId(uuid::Uuid::now_v7())).collect();
    for set in &sets {
        store.create_set(set).unwrap();
    }
    drop(store);
    let store = SqliteCasMds::open(root.path()).unwrap();
    assert_eq!(store.list_sets().unwrap().len(), 3);
    for set in &sets {
        assert!(store.list_containers(set).unwrap().is_empty());
    }
}

#[test]
fn migrate_all_creates_root_dirs_on_fresh_path() {
    let root = TempDir::new().unwrap();
    let _store = SqliteCasMds::open(root.path()).unwrap();
    assert!(root.path().join("containers").is_dir());
    assert!(root.path().join("blobs").is_dir());
    assert!(root.path().join("blobs.sqlite").is_file());
}

#[test]
fn stats_on_empty_root_has_zero_counters_and_unit_ratio() {
    let root = TempDir::new().unwrap();
    let store = SqliteCasMds::open(root.path()).unwrap();
    let stats = store.stats().unwrap();
    assert_eq!(
        (
            stats.set_count,
            stats.container_count,
            stats.item_count,
            stats.blob_count,
            stats.total_bytes
        ),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(stats.dedup_ratio, 1.0);
}

#[test]
fn stats_reports_counts_and_dedup() {
    let (_root, store, set, inbox) = populated();
    let blob = store.put_blob(b"hello").unwrap();
    store
        .add_item(
            &set,
            &blob,
            &[Membership {
                container: inbox,
                flags: Flags(0),
                added_at: 1,
            }],
        )
        .unwrap();
    let stats = store.stats().unwrap();
    assert_eq!(
        (
            stats.set_count,
            stats.container_count,
            stats.item_count,
            stats.blob_count,
            stats.total_bytes
        ),
        (1, 1, 2, 1, 5)
    );
    assert_eq!(stats.dedup_ratio, 2.0);
}

#[test]
fn stats_per_set_reports_both_sets() {
    let (_root, store, first, _) = populated();
    let second = SetId(uuid::Uuid::now_v7());
    store.create_set(&second).unwrap();
    store
        .create_container(&second, None, "INBOX", attrs())
        .unwrap();
    let rows = store.stats_per_set().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row.set_id == first
        && row.container_count == 1
        && row.item_count == 1
        && row.blob_count == 1
        && row.total_bytes == 5));
    assert!(rows.iter().any(|row| row.set_id == second
        && row.container_count == 1
        && row.item_count == 0
        && row.blob_count == 0
        && row.total_bytes == 0));
}

#[test]
fn verify_on_empty_root_reports_zero_blobs_zero_mismatches() {
    let root = TempDir::new().unwrap();
    let report = SqliteCasMds::open(root.path())
        .unwrap()
        .verify_blobs(VerifyScope::Full)
        .unwrap();
    assert_eq!((report.blobs_checked, report.mismatches), (0, 0));
}

#[test]
fn verify_clean_blobs_reports_zero_mismatches() {
    let (_root, store, _, _) = populated();
    let report = store.verify_blobs(VerifyScope::Full).unwrap();
    assert_eq!((report.blobs_checked, report.mismatches), (1, 0));
}

#[test]
fn verify_finds_mismatches_as_findings_and_reports_breakdown() {
    let (root, store, set, inbox) = populated();
    for (body, remove) in [
        (b"will-be-corrupted".as_slice(), false),
        (b"will-be-deleted".as_slice(), true),
    ] {
        let blob = store.put_blob(body).unwrap();
        store
            .add_item(
                &set,
                &blob,
                &[Membership {
                    container: inbox,
                    flags: Flags(0),
                    added_at: 1,
                }],
            )
            .unwrap();
        let hex = blob
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let path = root
            .path()
            .join("blobs")
            .join(&hex[..2])
            .join(&hex[2..4])
            .join(&hex);
        if remove {
            std::fs::remove_file(path).unwrap();
        } else {
            std::fs::write(path, b"different bytes").unwrap();
        }
    }
    let report = store.verify_blobs(VerifyScope::Full).unwrap();
    assert_eq!(
        (
            report.blobs_checked,
            report.mismatches,
            report.mismatches_hash,
            report.mismatches_missing
        ),
        (3, 2, 1, 1)
    );
}

#[test]
fn verify_since_scope_is_accepted() {
    let (_root, store, _, _) = populated();
    let report = store
        .verify_blobs(VerifyScope::Since(
            SystemTime::now() - Duration::from_secs(3600),
        ))
        .unwrap();
    assert_eq!((report.blobs_checked, report.mismatches), (1, 0));
}

#[test]
fn verify_container_scope_is_accepted() {
    let (_root, store, _, inbox) = populated();
    let report = store.verify_blobs(VerifyScope::Container(inbox)).unwrap();
    assert_eq!((report.blobs_checked, report.mismatches), (1, 0));
}

#[test]
fn gc_on_empty_root_reports_zero_deletions() {
    let root = TempDir::new().unwrap();
    let store = SqliteCasMds::open(root.path())
        .unwrap()
        .with_gc_quiescence(Duration::ZERO);
    let report = store.gc(false).unwrap();
    assert_eq!(
        (
            report.blobs_deleted,
            report.bytes_freed,
            report.candidates_pass1,
            report.skipped_re_referenced,
            report.skipped_re_touched,
            report.orphan_rows_swept,
            report.pending_rows_observed
        ),
        (0, 0, 0, 0, 0, 0, 0)
    );
}

#[test]
fn gc_dry_run_keeps_referenced_blob() {
    let (_root, store, _, _) = populated();
    let store = store.with_gc_quiescence(Duration::ZERO);
    let report = store.gc(true).unwrap();
    assert_eq!((report.blobs_deleted, report.bytes_freed), (0, 0));
    assert_eq!(store.stats().unwrap().blob_count, 1);
}

#[test]
fn rebuild_index_on_empty_root_reports_zero_counters() {
    let root = TempDir::new().unwrap();
    let report = SqliteCasMds::open(root.path())
        .unwrap()
        .rebuild_index()
        .unwrap();
    assert_eq!(
        (
            report.sets_scanned,
            report.items_indexed,
            report.blobs_indexed,
            report.orphan_blobs_found
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn rebuild_index_reports_set_item_blob_counts() {
    let (_root, store, _, _) = populated();
    let report = store.rebuild_index().unwrap();
    assert_eq!(
        (
            report.sets_scanned,
            report.items_indexed,
            report.blobs_indexed,
            report.orphan_blobs_found
        ),
        (1, 1, 1, 0)
    );
}

#[test]
fn export_writes_tarball_and_reports_counts() {
    let (root, store, set, _) = populated();
    let path = root.path().join("set.tar");
    let report = store.export_set(&set, &path).unwrap();
    assert_eq!(
        (report.set_id, report.item_count, report.blob_count),
        (set, 1, 1)
    );
    assert_eq!(report.bytes_written, std::fs::metadata(path).unwrap().len());
    assert!(report.bytes_written > 0);
}

#[test]
fn export_unknown_set_returns_set_not_found() {
    let root = TempDir::new().unwrap();
    let store = SqliteCasMds::open(root.path()).unwrap();
    let err = store
        .export_set(&SetId(uuid::Uuid::now_v7()), &root.path().join("set.tar"))
        .unwrap_err();
    assert!(matches!(err, Error::SetNotFound(_)));
}

#[test]
fn import_reports_counts_and_preserves_set() {
    let (root, store, set, _) = populated();
    let path = root.path().join("set.tar");
    let exported = store.export_set(&set, &path).unwrap();
    let dest = TempDir::new().unwrap();
    let target = SqliteCasMds::open(dest.path()).unwrap();
    let report = target.import_set(&path).unwrap();
    assert_eq!(
        (
            report.set_id,
            report.item_count,
            report.blob_count,
            report.bytes_read
        ),
        (set, 1, 1, exported.bytes_written)
    );
    assert_eq!(target.list_sets().unwrap(), vec![set]);
}

#[test]
fn import_missing_tarball_returns_error() {
    let root = TempDir::new().unwrap();
    let store = SqliteCasMds::open(root.path()).unwrap();
    assert!(store.import_set(&root.path().join("missing.tar")).is_err());
}

#[test]
fn import_existing_set_returns_error() {
    let (root, store, set, _) = populated();
    let path = root.path().join("set.tar");
    store.export_set(&set, &path).unwrap();
    assert!(store.import_set(&path).is_err());
}
