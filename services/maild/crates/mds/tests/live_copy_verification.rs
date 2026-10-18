// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opt-in verification of an offline MDS copy, including its WAL/SHM files.
//!
//! Set MDS_LIVE_COPY_ROOT and run with --ignored --nocapture. Prefer a read-only
//! mount: SQLITE_OPEN_READ_ONLY protects databases, but SQLite can otherwise
//! update an existing writable SHM file. Do not use immutable=1: it ignores WAL.
//! SqliteCasMds::open applies migrations, so this helper instead passes read-only
//! SQLite connections to the public container query API and uses blob::open.

use mds::{ItemId, SeqRange, blob, container};
use rusqlite::{Connection, OpenFlags};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn Error>>;
type Snapshot = BTreeMap<PathBuf, Option<blake3::Hash>>;

fn hash_file(path: &Path) -> io::Result<blake3::Hash> {
    hash_reader(File::open(path)?)
}

fn hash_reader(mut reader: impl Read) -> io::Result<blake3::Hash> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0; 65536];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(hasher.finalize());
        }
        hasher.update(&buffer[..count]);
    }
}

fn snapshot(root: &Path) -> io::Result<Snapshot> {
    fn visit(root: &Path, path: &Path, files: &mut Snapshot) -> io::Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            let relative = path
                .strip_prefix(root)
                .expect("under copy root")
                .to_path_buf();
            if kind.is_dir() {
                files.insert(relative, None);
                visit(root, &path, files)?;
            } else if kind.is_file() {
                files.insert(relative, Some(hash_file(&path)?));
            } else {
                return Err(io::Error::other(format!(
                    "copy contains a symlink or special file: {}",
                    path.display()
                )));
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

fn verify(root: &Path, before: &Snapshot) -> TestResult {
    let mut sets = fs::read_dir(root.join("containers"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    sets.sort();
    assert!(!sets.is_empty(), "no container sets in supplied copy");
    let mut sample_count = 0;
    let mut set_count = 0;
    for path in sets {
        if !path.is_dir() || !path.join("data.sqlite").is_file() {
            return Err(format!("unexpected set entry: {}", path.display()).into());
        }
        let set = path.file_name().expect("set directory").to_string_lossy();
        uuid::Uuid::parse_str(&set)?;
        let connection = Connection::open_with_flags(
            path.join("data.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        assert!(connection.is_readonly(rusqlite::DatabaseName::Main)?);
        let envelopes: u64 =
            connection.query_row("SELECT count(*) FROM mail_envelopes", [], |r| r.get(0))?;
        let item_count: u64 =
            connection.query_row("SELECT count(*) FROM item", [], |r| r.get(0))?;
        let ids = connection
            .prepare("SELECT id FROM item ORDER BY id")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<BTreeSet<_>, _>>()?;
        // Include unmoored/staging items rather than counting envelopes as items.
        for id in &ids {
            container::fetch_item_meta(&connection, &ItemId(uuid::Uuid::parse_str(id)?))?;
        }
        assert_eq!(ids.len() as u64, item_count);
        let containers = container::list_containers(&connection)?;
        let sql_containers: u64 =
            connection.query_row("SELECT count(*) FROM container", [], |r| r.get(0))?;
        assert_eq!(containers.len() as u64, sql_containers);
        let mut listed = BTreeSet::new();
        let mut memberships = BTreeMap::new();
        let mut membership_total = 0;
        let mut samples = BTreeMap::new();
        for info in containers {
            let items = container::list_items(&connection, &info.id, SeqRange::All)?;
            let expected: u64 = connection.query_row(
                "SELECT count(*) FROM membership WHERE container_id = ?1",
                [info.id.0.to_string()],
                |r| r.get(0),
            )?;
            assert_eq!(items.len() as u64, expected, "{set}/{}", info.id.0);
            assert_eq!(
                container::container_status(&connection, &info.id)?.exists,
                expected,
                "cached exists count for {set}/{}",
                info.id.0
            );
            membership_total += expected;
            memberships.insert(format!("{}:{}", info.name, info.id.0), expected);
            for item in &items {
                listed.insert(item.id.0.to_string());
            }
            if !items.is_empty() {
                for index in [0, items.len() / 2, items.len() - 1] {
                    let item = &items[index];
                    samples.insert(item.id.0.to_string(), item.meta.clone());
                }
            }
        }
        let sql_memberships: u64 =
            connection.query_row("SELECT count(*) FROM membership", [], |r| r.get(0))?;
        assert_eq!(membership_total, sql_memberships);
        assert!(listed.is_subset(&ids));
        println!(
            "SET {set} items={item_count} envelopes={envelopes} unmoored={} memberships={}",
            ids.len() - listed.len(),
            serde_json::to_string(&memberships)?
        );
        for (id, meta) in samples {
            let blobs_root = root.join("blobs");
            let path = blob::blob_path(&blobs_root, &meta.blob_hash);
            let hash = hash_reader(blob::open(&blobs_root, &meta.blob_hash)?)?;
            assert_eq!(hash.as_bytes(), &meta.blob_hash, "blob hash for {set}/{id}");
            assert_eq!(
                path.file_name().and_then(|name| name.to_str()),
                Some(hash.to_hex().as_str()),
                "CAS filename for {set}/{id}"
            );
            assert_eq!(fs::metadata(&path)?.len(), meta.size_bytes);
            println!("BLOB set={set} item={id} hash={hash} OK");
            sample_count += 1;
        }
        set_count += 1;
    }
    let cas_count = before
        .iter()
        .filter(|(path, hash)| path.starts_with("blobs") && hash.is_some())
        .count();
    println!(
        "SUMMARY sets={set_count} CAS_FILES={cas_count} samples={sample_count} \
         sqlite=SQLITE_OPEN_READ_ONLY api=mds::container,mds::blob"
    );
    Ok(())
}

#[test]
#[ignore = "requires an offline copy at MDS_LIVE_COPY_ROOT; reads every file"]
fn live_copy_verification() -> TestResult {
    let root = PathBuf::from(
        std::env::var_os("MDS_LIVE_COPY_ROOT")
            .ok_or("set MDS_LIVE_COPY_ROOT to an offline MDS copy, preferably mounted read-only")?,
    )
    .canonicalize()?;
    let before = snapshot(&root)?;
    let result = verify(&root, &before);
    let after = snapshot(&root)?;
    assert_eq!(before, after, "verification changed the copied store");
    println!("STORE_UNCHANGED files, directories and BLAKE3 hashes match");
    result
}
