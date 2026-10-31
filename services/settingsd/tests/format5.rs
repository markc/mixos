// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Store format 5 end to end (design §4.2-§4.4, S6): a format-4 profile as
//! today's binary writes it migrates on open with its record kept, serves,
//! and downgrades by the reverse conversion and by `--from-kept`, each
//! candidate passing the format-4 seed; interrupted migrations and
//! downgrades resume; and the mirror's sealed cache is refused and rebuilt,
//! never served, when its seal fails.
use serde_json::json;
use settings::*;
use settingsd::authority::Authority;
use settingsd::format4::Record;
use settingsd::migration::{KEPT, MigrationRecord, RECORD};
use settingsd::mirror::{self, Cache, Origin};
use settingsd::store::{Accepted, Store, Stored};
use std::collections::BTreeMap;
use std::path::Path;

const PRIMARY: &str = "desktop.conf.mix";
const BACKUP: &str = "desktop.previous.conf.mix";

fn binding() -> Binding {
    Binding {
        instance: "example".into(),
        profile: "default".into(),
    }
}

/// A long-used format-4 profile: an authored look, panels, a terminal and
/// a mail account, at revision 245 with a receipt, and the backup one
/// commit behind. Written by the frozen format-4 code, which is today's
/// binary's.
fn format4_profile(root: &Path) -> (String, String) {
    let mut desktop = Desktop::default();
    desktop.appearance.scheme = "forest".into();
    desktop.appearance.style = Some("adwaita".into());
    desktop.appearance.contrast = "high".into();
    desktop.appearance.decorations = "ssd".into();
    desktop.appearance.font_size = Some(12.0);
    desktop.shell.panels.get_mut("bottom").unwrap().mode = "hidden".into();
    desktop.terminal.font.size = 15.0;
    desktop.email = serde_json::from_value(json!({"default_account":"acct-1",
        "accounts":[{"id":"acct-1","name":"Home","url":"https://mail.example.org",
        "email":"me@example.org"}]}))
    .unwrap();
    let mut record = Record::initial(binding(), desktop).unwrap();
    record.revision = Revision(244);
    record.design_revision = Revision(244);
    record.seal().unwrap();
    let backup = strict::to_string_pretty(&record).unwrap();
    record.revision = Revision(245);
    record.receipts.push(Receipt {
        operation_id: "font-size".into(),
        request_digest: "a".repeat(64),
        incarnation: record.incarnation.clone(),
        revision: Revision(245),
        outcome: Outcome::Changed,
    });
    record.seal().unwrap();
    let primary = strict::to_string_pretty(&record).unwrap();
    for (name, text) in [(PRIMARY, &primary), (BACKUP, &backup)] {
        std::fs::write(root.join(name), text).unwrap();
    }
    (primary, backup)
}

/// What the format-4 settingsd's `seed` does with a staged record: parse
/// it as format 4 (a format-5 record does not parse), verify its seal,
/// upgrade within format 4 and re-derive its effective map.
fn format4_seed(stage: &Path, binding: &Binding) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(stage.join(PRIMARY))?;
    let record: Record = strict::from_str(&text)?;
    record.check(binding)?;
    let (record, _) = record.upgrade()?;
    record.effective()?;
    Ok(())
}

fn read(root: &Path, name: &str) -> String {
    std::fs::read_to_string(root.join(name)).unwrap()
}

fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_file())
        .map(|entry| {
            (
                entry.file_name().into_string().unwrap(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

fn copy_files(from: &Path, to: &Path) {
    for (name, bytes) in files(from) {
        std::fs::write(to.join(name), bytes).unwrap();
    }
}

fn apply(authority: &mut Authority, id: &str, changes: serde_json::Value) -> serde_json::Value {
    authority
        .apply(ApplyRequest {
            binding: binding(),
            expected_incarnation: authority.accepted.incarnation.clone(),
            expected_revision: authority.accepted.revision,
            operation_id: id.into(),
            changes: serde_json::from_value(changes).unwrap(),
            reset: vec![],
            request_digest: None,
        })
        .unwrap()
}

#[test]
fn a_format4_profile_migrates_on_open_with_its_record_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (primary, _) = format4_profile(dir.path());
    let (store, accepted) = Store::open(dir.path(), &binding()).unwrap();
    assert!(store.migrated && !store.restored);
    assert_eq!(read(dir.path(), KEPT), primary, "kept byte for byte");
    let old: Record = strict::from_str(&primary).unwrap();
    assert_eq!(accepted.incarnation, old.incarnation);
    assert_eq!(accepted.revision, Revision(246));
    assert_eq!(accepted.design_revision, Revision(245));
    let receipt = accepted.receipts.last().unwrap();
    assert_eq!(receipt.operation_id, "migrate-v2");
    assert_eq!(receipt.request_digest, source_digest(&primary));
    assert_eq!(accepted.receipts[0].operation_id, "font-size");
    // The table's result (§4.1, F2).
    assert_eq!(accepted.values["appearance.scheme"], "forest");
    assert_eq!(accepted.values["window.decorations"], "ssd");
    assert_eq!(accepted.values["shell.panels.bottom.autohide"], true);
    assert!(
        !accepted.values.contains_key("appearance.roundness"),
        "adwaita is round"
    );
    // The evidence beside it.
    let record: MigrationRecord = strict::from_str(&read(dir.path(), RECORD)).unwrap();
    assert_eq!(record.source_digest, source_digest(&primary));
    assert_eq!(record.old_content_digest, old.content_digest);
    assert_eq!(record.new_content_digest, accepted.content_digest);
    assert_eq!((record.from_format, record.to_format), (4, 5));
    assert!(record.report["dropped"].is_array());
    // The backup migrated too; both serve.
    let Stored::Current(backup) = Stored::parse(&read(dir.path(), BACKUP)).unwrap() else {
        panic!("the backup is format 5");
    };
    backup.check(&binding()).unwrap();
    assert_eq!(backup.revision, Revision(245));
    let authority = Authority::new(store, accepted).unwrap();
    assert_eq!(
        authority.snapshot.desktop.shell.panels["bottom"].mode,
        "hidden"
    );
    assert_eq!(authority.snapshot.effective["desktop"].scheme, "forest");
    drop(authority);
    // A second open changes nothing.
    let before = files(dir.path());
    let (store, _) = Store::open(dir.path(), &binding()).unwrap();
    assert!(!store.migrated);
    assert_eq!(files(dir.path()), before);
}

#[test]
fn a_migrated_profile_serves_then_downgrades_by_the_reverse_conversion() {
    let dir = tempfile::tempdir().unwrap();
    format4_profile(dir.path());
    let (store, accepted) = Store::open(dir.path(), &binding()).unwrap();
    let mut authority = Authority::new(store, accepted).unwrap();
    // A change since the migration, with a format-4 home.
    apply(&mut authority, "light", json!({"appearance.mode":"light"}));
    let served = authority.accepted.clone();
    drop(authority);
    let five = read(dir.path(), PRIMARY);
    let report =
        settingsd::downgrade::downgrade(dir.path(), &binding(), 4, false, format4_seed).unwrap();
    assert_eq!(report["status"], "downgraded");
    assert_eq!(report["route"], "reverse");
    // The live files are the candidate, and the format-4 seed takes them.
    format4_seed(dir.path(), &binding()).unwrap();
    let record: Record = strict::from_str(&read(dir.path(), PRIMARY)).unwrap();
    assert_eq!(record.format, 4);
    assert_eq!(read(dir.path(), BACKUP), read(dir.path(), PRIMARY));
    assert_eq!(
        record.desktop.appearance.mode, "light",
        "the change survives"
    );
    assert_eq!(record.desktop.appearance.scheme, "forest");
    assert_eq!(record.desktop.shell.panels["bottom"].mode, "hidden");
    assert_eq!(record.desktop.terminal.font.size, 15.0);
    assert_eq!(record.incarnation, served.incarnation);
    assert_eq!(record.revision, Revision(served.revision.0 + 1));
    assert_eq!(record.receipts.last().unwrap().operation_id, "downgrade-4");
    // The format-5 files are kept.
    assert_eq!(read(dir.path(), "desktop.format5.conf.mix"), five);
    assert!(
        dir.path()
            .join("desktop.format5.previous.conf.mix")
            .exists()
    );
    assert!(!dir.path().join("downgrade-stage").exists());
    // Again: done, nothing changes.
    let before = files(dir.path());
    let again =
        settingsd::downgrade::downgrade(dir.path(), &binding(), 4, false, format4_seed).unwrap();
    assert_eq!(again["status"], "unchanged");
    assert_eq!(files(dir.path()), before);
    // A format-5 build opens it again: a second migration, the first kept
    // record kept aside as superseded.
    let (_, reopened) = Store::open(dir.path(), &binding()).unwrap();
    assert_eq!(reopened.values["appearance.mode"], "light");
    assert!(
        files(dir.path())
            .keys()
            .any(|name| name.starts_with("desktop.superseded-") || name.contains(".superseded-")),
        "{:?}",
        files(dir.path()).keys().collect::<Vec<_>>()
    );
}

#[test]
fn a_v2_only_value_refuses_the_reverse_route_and_from_kept_restores_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let (primary, _) = format4_profile(dir.path());
    let (store, accepted) = Store::open(dir.path(), &binding()).unwrap();
    let mut authority = Authority::new(store, accepted).unwrap();
    apply(
        &mut authority,
        "accent",
        json!({"appearance.accent":"#3584e4","appearance.mode":"light"}),
    );
    drop(authority);
    let before = files(dir.path());
    let refused = settingsd::downgrade::downgrade(dir.path(), &binding(), 4, false, format4_seed)
        .unwrap_err()
        .to_string();
    assert!(refused.contains("appearance.accent"), "{refused}");
    assert!(refused.contains("--from-kept"), "{refused}");
    assert_eq!(files(dir.path()), before, "a refusal writes nothing");
    let report =
        settingsd::downgrade::downgrade(dir.path(), &binding(), 4, true, format4_seed).unwrap();
    assert_eq!(report["route"], "from_kept");
    assert_eq!(
        read(dir.path(), PRIMARY),
        primary,
        "byte for byte the original"
    );
    assert_eq!(read(dir.path(), BACKUP), primary);
    format4_seed(dir.path(), &binding()).unwrap();
    let lost: Vec<&str> = report["lost"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert_eq!(lost, ["appearance.accent", "appearance.mode"]);
}

#[test]
fn the_derived_scheme_blocks_the_reverse_route() {
    // A fresh format-5 profile draws `mixos`, which format 4 has not got.
    let dir = tempfile::tempdir().unwrap();
    drop(Store::create(dir.path(), binding(), Default::default()).unwrap());
    let refused = settingsd::downgrade::downgrade(dir.path(), &binding(), 4, false, format4_seed)
        .unwrap_err()
        .to_string();
    assert!(refused.contains("appearance.scheme"), "{refused}");
    // With nothing kept there is no other route.
    let refused = settingsd::downgrade::downgrade(dir.path(), &binding(), 4, true, format4_seed)
        .unwrap_err()
        .to_string();
    assert!(refused.contains(KEPT), "{refused}");
    assert!(
        settingsd::downgrade::downgrade(dir.path(), &binding(), 3, true, format4_seed).is_err()
    );
}

#[test]
fn a_candidate_the_target_refuses_leaves_the_profile_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    format4_profile(dir.path());
    drop(Store::open(dir.path(), &binding()).unwrap());
    let before = files(dir.path());
    let refuse = |_: &Path, _: &Binding| anyhow::bail!("target refused");
    assert!(settingsd::downgrade::downgrade(dir.path(), &binding(), 4, false, refuse).is_err());
    assert_eq!(files(dir.path()), before);
    assert!(!dir.path().join("downgrade-stage").exists());
}

#[test]
fn an_interrupted_migration_resumes_to_the_same_record() {
    let whole = tempfile::tempdir().unwrap();
    let (primary, backup) = format4_profile(whole.path());
    drop(Store::open(whole.path(), &binding()).unwrap());
    let done = files(whole.path());
    // Cut after each write in turn: kept; evidence; backup.
    for written in [&[KEPT][..], &[KEPT, RECORD], &[KEPT, RECORD, BACKUP]] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(PRIMARY), &primary).unwrap();
        std::fs::write(dir.path().join(BACKUP), &backup).unwrap();
        for name in written {
            std::fs::write(dir.path().join(name), &done[*name]).unwrap();
        }
        let (store, _) = Store::open(dir.path(), &binding()).unwrap();
        assert!(store.migrated);
        drop(store);
        let mut resumed = files(dir.path());
        resumed.remove("writer.lock");
        let mut expected = done.clone();
        expected.remove("writer.lock");
        assert_eq!(resumed, expected, "after {written:?}");
    }
}

#[test]
fn an_interrupted_downgrade_resumes_to_the_same_record() {
    let migrated = tempfile::tempdir().unwrap();
    format4_profile(migrated.path());
    drop(Store::open(migrated.path(), &binding()).unwrap());
    let whole = tempfile::tempdir().unwrap();
    copy_files(migrated.path(), whole.path());
    settingsd::downgrade::downgrade(whole.path(), &binding(), 4, false, format4_seed).unwrap();
    let done = files(whole.path());
    // Cut after the format-5 files are kept; after the backup; after the
    // primary (finished, but not yet reported).
    for written in [
        &[
            "desktop.format5.conf.mix",
            "desktop.format5.previous.conf.mix",
        ][..],
        &[
            "desktop.format5.conf.mix",
            "desktop.format5.previous.conf.mix",
            BACKUP,
        ],
        &[
            "desktop.format5.conf.mix",
            "desktop.format5.previous.conf.mix",
            BACKUP,
            PRIMARY,
        ],
    ] {
        let dir = tempfile::tempdir().unwrap();
        copy_files(migrated.path(), dir.path());
        for name in written {
            std::fs::write(dir.path().join(name), &done[*name]).unwrap();
        }
        settingsd::downgrade::downgrade(dir.path(), &binding(), 4, false, format4_seed).unwrap();
        assert_eq!(files(dir.path()), done, "after {written:?}");
    }
}

#[test]
fn a_mirror_cache_that_fails_its_seal_is_refused_and_rebuilt_never_served() {
    let host = tempfile::tempdir().unwrap();
    let (store, accepted) = Store::create(
        host.path(),
        binding(),
        BTreeMap::from([("appearance.scheme".into(), json!("forest"))]),
    )
    .unwrap();
    drop(store);
    let good = Cache::of(&accepted, "host").unwrap();
    let desk = tempfile::tempdir().unwrap();
    let root = desk.path().join("mirror");
    let fetched = || Ok(good.clone());
    let unreachable = || -> anyhow::Result<Cache> { anyhow::bail!("authority unreachable") };
    // No cache: built from the authority, no writer lock.
    let (cache, origin) = mirror::open(&root, &binding(), "host", fetched).unwrap();
    assert_eq!(
        (cache.clone(), origin),
        (good.clone(), Origin::Rebuilt { refused: None })
    );
    assert!(!root.join("writer.lock").exists());
    // A cache that verifies is served without the authority.
    let (cache, origin) = mirror::open(&root, &binding(), "host", unreachable).unwrap();
    assert_eq!((cache, origin), (good.clone(), Origin::Cached));
    // Tampered: refused, set aside and rebuilt, never served.
    let path = root.join(mirror::CACHE);
    let mut tampered: serde_json::Value =
        strict::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    tampered["values"]["appearance.scheme"] = json!("ocean");
    std::fs::write(&path, strict::to_string_pretty(&tampered).unwrap()).unwrap();
    let (cache, origin) = mirror::open(&root, &binding(), "host", fetched).unwrap();
    assert_eq!(cache, good);
    let Origin::Rebuilt {
        refused: Some(aside),
    } = origin
    else {
        panic!("the tampered cache was refused");
    };
    assert!(root.join(&aside).exists(), "the evidence is kept");
    // Tampered with the authority away: refused, and nothing is served.
    std::fs::write(&path, strict::to_string_pretty(&tampered).unwrap()).unwrap();
    assert!(mirror::open(&root, &binding(), "host", unreachable).is_err());
    assert!(!path.exists());
    // A fetched cache that fails the check is not written either.
    let mut bad = good.clone();
    bad.revision = Revision(99);
    assert!(mirror::open(&root, &binding(), "host", || Ok(bad)).is_err());
    assert!(!path.exists());
    // Another authority's cache is refused.
    let (_, _) = mirror::open(&root, &binding(), "host", fetched).unwrap();
    let (_, origin) =
        mirror::open(&root, &binding(), "other", || Cache::of(&accepted, "other")).unwrap();
    assert!(matches!(origin, Origin::Rebuilt { refused: Some(_) }));
    // A profile's root is not a mirror's.
    assert!(mirror::open(host.path(), &binding(), "host", fetched).is_err());
}

#[test]
fn a_mirror_serves_reads_and_refuses_writes_until_s7() {
    let host = tempfile::tempdir().unwrap();
    let (store, accepted) = Store::create(host.path(), binding(), Default::default()).unwrap();
    let authority = Authority::new(store, accepted.clone()).unwrap();
    let cache = Cache::of(&accepted, "host").unwrap();
    let mut mirror = Authority::mirror(&cache).unwrap();
    assert_eq!(mirror.snapshot, authority.snapshot);
    let status = mirror.status(&binding(), None).unwrap();
    assert_eq!(
        (&status["role"], &status["authority"], &status["stale"]),
        (&json!("mirror"), &json!("host"), &json!(true))
    );
    let refused = mirror
        .apply(ApplyRequest {
            binding: binding(),
            expected_incarnation: accepted.incarnation.clone(),
            expected_revision: accepted.revision,
            operation_id: "through-mirror".into(),
            changes: BTreeMap::from([("appearance.mode".into(), json!("light"))]),
            reset: vec![],
            request_digest: None,
        })
        .unwrap_err();
    assert_eq!(refused["status"], "authority_unreachable");
    assert_eq!(refused["node"], "host");
}

#[test]
fn the_dry_run_writes_nothing_and_the_backdrop_imports_once() {
    let dir = tempfile::tempdir().unwrap();
    format4_profile(dir.path());
    let backdrop = dir.path().join("backdrop.mx");
    std::fs::write(
        &backdrop,
        "{schema:1, default:{colour:\"#1e1e2e\", image:\"/home/user/walls/hills.jpg\"}}",
    )
    .unwrap();
    let before = files(dir.path());
    let dry = settingsd::migration::dry_run(dir.path(), &binding(), Some(&backdrop)).unwrap();
    assert_eq!(dry["status"], "would_migrate", "{dry}");
    assert_eq!(dry["receipt"]["operation_id"], "migrate-v2");
    assert_eq!(dry["revision"], "246");
    assert_eq!(dry["backdrop"]["status"], "would_import", "{dry}");
    assert_eq!(
        dry["backdrop"]["wallpaper"]["wallpaper.image"],
        "/home/user/walls/hills.jpg"
    );
    assert_eq!(files(dir.path()), before, "the dry run wrote nothing");
    // The import: migrates on open, writes one fenced revision, renames.
    let imported =
        settingsd::migration::import_backdrop(dir.path(), &binding(), &backdrop).unwrap();
    assert_eq!(imported["status"], "imported", "{imported}");
    assert!(!backdrop.exists());
    assert!(dir.path().join("backdrop.mx.migrated").exists());
    let (_, accepted) = Store::open(dir.path(), &binding()).unwrap();
    assert_eq!(
        accepted.values["wallpaper.image"],
        "/home/user/walls/hills.jpg"
    );
    assert_eq!(accepted.revision, Revision(247));
    let again = settingsd::migration::import_backdrop(dir.path(), &binding(), &backdrop).unwrap();
    assert_eq!(again["status"], "already_imported");
    // A format-5 profile dry-runs as current.
    let current = settingsd::migration::dry_run(dir.path(), &binding(), None).unwrap();
    assert_eq!(current["status"], "current");
}

#[test]
fn the_command_line_migrates_and_downgrades_through_the_target_binary() {
    let dir = tempfile::tempdir().unwrap();
    format4_profile(dir.path());
    let run = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_settingsd"))
            .args(args)
            .args(["--instance", "example", "--root"])
            .arg(dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    assert_eq!(run(&["migrate", "--dry-run"])["status"], "would_migrate");
    assert_eq!(run(&["migrate"])["status"], "migrated");
    assert_eq!(run(&["migrate"])["status"], "current");
    // The target here is this binary, whose seed migrates the staged
    // candidate again; the format-4 binary's own seed is S9's acceptance.
    let downgraded = run(&[
        "downgrade",
        "--to",
        "4",
        "--target",
        env!("CARGO_BIN_EXE_settingsd"),
    ]);
    assert_eq!(downgraded["status"], "downgraded", "{downgraded}");
    format4_seed(dir.path(), &binding()).unwrap();
    assert_eq!(run(&["retire"])["status"], "retired");
    assert!(dir.path().join("desktop.conf.mix.retired-example").exists());
    assert!(!dir.path().join(PRIMARY).exists());
}

/// The kept record of a migrated profile is what `Record` reads.
#[test]
fn the_kept_record_reads_as_format_4() {
    let dir = tempfile::tempdir().unwrap();
    format4_profile(dir.path());
    let (_, accepted): (Store, Accepted) = Store::open(dir.path(), &binding()).unwrap();
    let kept: Record = strict::from_str(&read(dir.path(), KEPT)).unwrap();
    kept.check(&binding()).unwrap();
    assert_eq!(kept.incarnation, accepted.incarnation);
}

/// The format-4 binary's `seed` (main.rs before format 5): without
/// `--allow-create` it opens; with it, a root with no `writer.lock` takes
/// the creation path, which refuses a root that already holds a primary.
fn format4_seed_cli(stage: &Path, binding: &Binding, allow_create: bool) -> anyhow::Result<()> {
    if allow_create && !stage.join("writer.lock").exists() {
        anyhow::ensure!(
            !stage.join(PRIMARY).exists(),
            "profile already initialised; initialise never overwrites accepted/backup data"
        );
    }
    format4_seed(stage, binding)
}

/// The directory downgrade stages is one the format-4 seed opens, with or
/// without `--allow-create`, for both routes.
#[test]
fn the_staged_directory_is_one_the_format4_seed_opens() {
    for from_kept in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        format4_profile(dir.path());
        drop(Store::open(dir.path(), &binding()).unwrap());
        let seen = std::cell::RefCell::new(Vec::new());
        let prove = |stage: &Path, binding: &Binding| {
            *seen.borrow_mut() = files(stage).into_keys().collect();
            format4_seed_cli(stage, binding, false)?;
            format4_seed_cli(stage, binding, true)
        };
        settingsd::downgrade::downgrade(dir.path(), &binding(), 4, from_kept, prove).unwrap();
        assert_eq!(*seen.borrow(), [PRIMARY, BACKUP, "writer.lock"]);
    }
}

#[test]
fn from_kept_works_when_the_format5_primary_is_damaged() {
    let dir = tempfile::tempdir().unwrap();
    let (primary, _) = format4_profile(dir.path());
    drop(Store::open(dir.path(), &binding()).unwrap());
    std::fs::write(dir.path().join(PRIMARY), "{ broken").unwrap();
    // The reverse route needs the record; the kept route does not.
    assert!(
        settingsd::downgrade::downgrade(dir.path(), &binding(), 4, false, format4_seed).is_err()
    );
    let report =
        settingsd::downgrade::downgrade(dir.path(), &binding(), 4, true, format4_seed).unwrap();
    assert_eq!(report["status"], "downgraded");
    assert_eq!(report["lost"], "unavailable");
    assert_eq!(read(dir.path(), PRIMARY), primary);
    format4_seed(dir.path(), &binding()).unwrap();
    assert!(
        files(dir.path())
            .iter()
            .any(|(name, bytes)| name.starts_with("corrupt-") && bytes == b"{ broken"),
        "the damaged primary is kept"
    );
    // With the primary gone altogether, the kept record still serves.
    std::fs::remove_file(dir.path().join(PRIMARY)).unwrap();
    let report =
        settingsd::downgrade::downgrade(dir.path(), &binding(), 4, true, format4_seed).unwrap();
    assert_eq!(report["lost"], "unavailable");
    assert_eq!(read(dir.path(), PRIMARY), primary);
}

#[test]
fn an_interrupted_from_kept_downgrade_resumes_to_the_same_record() {
    let migrated = tempfile::tempdir().unwrap();
    format4_profile(migrated.path());
    let (store, accepted) = Store::open(migrated.path(), &binding()).unwrap();
    let mut authority = Authority::new(store, accepted).unwrap();
    apply(
        &mut authority,
        "accent",
        json!({"appearance.accent":"#3584e4"}),
    );
    drop(authority);
    let whole = tempfile::tempdir().unwrap();
    copy_files(migrated.path(), whole.path());
    settingsd::downgrade::downgrade(whole.path(), &binding(), 4, true, format4_seed).unwrap();
    let done = files(whole.path());
    for written in [
        &[
            "desktop.format5.conf.mix",
            "desktop.format5.previous.conf.mix",
        ][..],
        &[
            "desktop.format5.conf.mix",
            "desktop.format5.previous.conf.mix",
            BACKUP,
        ],
        &[
            "desktop.format5.conf.mix",
            "desktop.format5.previous.conf.mix",
            BACKUP,
            PRIMARY,
        ],
    ] {
        let dir = tempfile::tempdir().unwrap();
        copy_files(migrated.path(), dir.path());
        for name in written {
            std::fs::write(dir.path().join(name), &done[*name]).unwrap();
        }
        settingsd::downgrade::downgrade(dir.path(), &binding(), 4, true, format4_seed).unwrap();
        assert_eq!(files(dir.path()), done, "after {written:?}");
    }
}

/// Rebind reports the record as stored: a format-4 profile stays format 4
/// at its revision, and migrates on its next open.
#[test]
fn rebind_reports_the_stored_record() {
    let dir = tempfile::tempdir().unwrap();
    format4_profile(dir.path());
    let renamed = Binding {
        instance: "renamed".into(),
        profile: "default".into(),
    };
    let (identity, changed) = Store::rebind(dir.path(), &binding(), renamed.clone()).unwrap();
    assert!(changed);
    assert_eq!(
        (identity.format, identity.revision, &identity.binding),
        (4, Revision(245), &renamed)
    );
    let (store, accepted) = Store::open(dir.path(), &renamed).unwrap();
    assert!(store.migrated);
    assert_eq!(accepted.revision, Revision(246));
}

#[test]
fn a_mirror_write_is_checked_against_the_mirrors_own_binding() {
    let host = tempfile::tempdir().unwrap();
    let (store, accepted) = Store::create(host.path(), binding(), Default::default()).unwrap();
    drop(store);
    let desk = tempfile::tempdir().unwrap();
    let root = desk.path().join("mirror");
    let cache = Cache::of(&accepted, "host").unwrap();
    assert!(mirror::write(&root, &binding(), "other", &cache).is_err());
    let other = Binding {
        instance: "other".into(),
        profile: "default".into(),
    };
    assert!(mirror::write(&root, &other, "host", &cache).is_err());
    assert!(!root.join(mirror::CACHE).exists());
    mirror::write(&root, &binding(), "host", &cache).unwrap();
    let unreachable = || -> anyhow::Result<Cache> { anyhow::bail!("authority unreachable") };
    let (served, origin) = mirror::open(&root, &binding(), "host", unreachable).unwrap();
    assert_eq!((served, origin), (cache, Origin::Cached));
}
