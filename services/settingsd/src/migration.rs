// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Format 4 to 5 (design §4.2): the store's half of the migration whose
//! table is [`settings::migrate`]. On open, after the seal verifies, the
//! format-4 primary is kept byte for byte as [`KEPT`], `migration.mx`
//! records what the table did, and the backup then the primary are
//! rewritten as format 5. Every step is idempotent on its output, so an
//! interrupted migration finishes on the next open. Also the
//! `settingsd migrate` command: the dry run and the one-time backdrop
//! import.
use crate::format4::Record;
use crate::store::{
    self, Accepted, BACKUP, FORMAT, PRIMARY, Store, Stored, canonical, encode, integral,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use settings::*;
use std::path::Path;

/// The format-4 record as it was before its migration, never written again
/// by format 5.
pub const KEPT: &str = "desktop.format4.conf.mix";
/// What the last migration did.
pub const RECORD: &str = "migration.mx";
/// The receipt the migration leaves in the record.
pub const RECEIPT: &str = "migrate-v2";
/// The operation id of the backdrop import, followed by its digest.
const IMPORT: &str = "import-backdrop";

/// `migration.mx`: one migration's evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationRecord {
    pub schema: u32,
    pub from_format: u32,
    pub to_format: u32,
    /// The digest of the kept record's bytes, which the receipt carries.
    pub source_digest: String,
    /// The format-4 record's seal and the format-5 record's.
    pub old_content_digest: String,
    pub new_content_digest: String,
    pub revision: Revision,
    /// [`settings::migrate::MigrationReport`]: what was dropped or changed,
    /// each entry with its stable code.
    pub report: Value,
}

/// A migration worked out, nothing written.
pub struct Planned {
    pub accepted: Accepted,
    pub record: MigrationRecord,
    pub receipt: Receipt,
}

/// The format-4 record `record`, read from `text` and verified, at format
/// 5: upgraded within format 4 first, then through the table. The
/// incarnation and receipts are kept; revision and design revision advance
/// by one and a [`RECEIPT`] receipt records the step.
pub fn plan(record: Record, text: &str) -> anyhow::Result<Planned> {
    let from_format = record.format;
    let (record, _) = record.upgrade()?;
    record.effective()?;
    let migrated = settings::migrate::migrate(&record.desktop).map_err(|failed| {
        anyhow::anyhow!(
            "format-4 record cannot migrate: {}",
            serde_json::to_string(&failed).unwrap_or_default()
        )
    })?;
    let next = |revision: Revision| {
        revision
            .0
            .checked_add(1)
            .map(Revision)
            .ok_or_else(|| anyhow::anyhow!("accepted revision exhausted"))
    };
    let revision = next(record.revision)?;
    let source_digest = settings::source_digest(text);
    let receipt = Receipt {
        operation_id: RECEIPT.into(),
        request_digest: source_digest.clone(),
        incarnation: record.incarnation.clone(),
        revision,
        outcome: Outcome::Changed,
    };
    // A profile migrated before (and downgraded since) carries the old one.
    let mut receipts: Vec<Receipt> = record
        .receipts
        .into_iter()
        .filter(|r| r.operation_id != RECEIPT)
        .collect();
    receipts.push(receipt.clone());
    if receipts.len() > MAX_RECEIPTS {
        receipts.remove(0);
    }
    let mut accepted = Accepted {
        schema: SCHEMA,
        format: FORMAT,
        binding: record.binding,
        incarnation: record.incarnation,
        revision,
        design_revision: next(record.design_revision)?,
        values: canonical(migrated.values),
        embedded_source: record.embedded_source,
        effective_digest: String::new(),
        receipts,
        content_digest: String::new(),
    };
    let (effective, _) = accepted.resolve()?;
    accepted.effective_digest = settings::digest(&effective)?;
    accepted.seal()?;
    accepted.check(&accepted.binding)?;
    let record = MigrationRecord {
        schema: 1,
        from_format,
        to_format: FORMAT,
        source_digest,
        old_content_digest: record.content_digest,
        new_content_digest: accepted.content_digest.clone(),
        revision,
        report: serde_json::to_value(&migrated.report)?,
    };
    Ok(Planned {
        accepted,
        record,
        receipt,
    })
}

/// Migrate the verified older primary `record`, stored as `text`: keep it,
/// write the evidence, then the backup, then the primary. Each write is the
/// synced atomic replacement, and each is skipped when its file already
/// holds its output, so a migration interrupted at any cut resumes.
pub(crate) fn on_open(
    store: &Store,
    binding: &Binding,
    record: Record,
    text: &str,
) -> anyhow::Result<Accepted> {
    let planned = plan(record, text)?;
    store.keep(KEPT, text.as_bytes())?;
    store.keep(RECORD, &encode(&planned.record)?)?;
    store.migrate_backup(binding)?;
    store.replace(PRIMARY, &encode(&planned.accepted)?)?;
    eprintln!(
        "settingsd: migrated format {} to {FORMAT} at revision {}; {KEPT} keeps the old record",
        planned.record.from_format, planned.accepted.revision.0
    );
    Ok(planned.accepted)
}

/// `settingsd migrate --dry-run`: the migration's table result and receipt,
/// and with `backdrop` the import's, from the profile as stored. Takes no
/// lock and writes nothing, so it runs beside a serving settingsd.
pub fn dry_run(root: &Path, binding: &Binding, backdrop: Option<&Path>) -> anyhow::Result<Value> {
    let directory = store::open_directory(root)?;
    let text = String::from_utf8(store::read_bytes(store::open_in(
        &directory,
        PRIMARY,
        libc::O_RDONLY,
    )?)?)?;
    let stored = Stored::parse(&text)?;
    stored.check(binding)?;
    let mut result = match stored {
        Stored::Current(accepted) => {
            accepted.effective()?;
            json!({"status":"current","format":FORMAT,"revision":accepted.revision,"values":accepted.values})
        }
        Stored::Format4(record) => {
            let from_format = record.format;
            match plan(*record, &text) {
                Ok(planned) => json!({
                    "status":"would_migrate","from_format":from_format,"to_format":FORMAT,
                    "revision":planned.accepted.revision,"report":planned.record.report,
                    "receipt":planned.receipt,"migration":planned.record,
                    "values":planned.accepted.values,
                    "writes":[KEPT, RECORD, BACKUP, PRIMARY],
                }),
                Err(error) => json!({"status":"refused","message":error.to_string()}),
            }
        }
    };
    if let Some(path) = backdrop
        && result["status"] != "refused"
    {
        let values: registry::Values = serde_json::from_value(result["values"].clone())?;
        result["backdrop"] = match read_backdrop(path) {
            Ok((document, _)) => match settings::migrate::import_backdrop(&values, &document) {
                Ok(imported) => json!({"status":"would_import","report":imported.report,
                    "wallpaper":wallpaper(&imported.values)}),
                Err(failed) => json!({"status":"refused","failed":failed}),
            },
            Err(error) => json!({"status":"refused","message":error.to_string()}),
        };
        if result["backdrop"]["status"] == "refused" {
            result["status"] = "refused".into();
        }
    }
    Ok(result)
}

/// `settingsd migrate --import-backdrop PATH`, beside the authority with
/// settingsd stopped: the profile is opened (and so migrated), the
/// document's wallpaper replaces `wallpaper.*` as one fenced write with a
/// receipt, and the file is renamed `PATH.migrated`. Run again after an
/// interruption, the write replays from its receipt and the rename
/// finishes.
pub fn import_backdrop(root: &Path, binding: &Binding, path: &Path) -> anyhow::Result<Value> {
    let done = migrated_name(path)?;
    let (store, accepted) = Store::open(root, binding)?;
    if !path.exists() && done.exists() {
        return Ok(json!({"status":"already_imported","document":done}));
    }
    let (document, text) = read_backdrop(path)?;
    let imported =
        settings::migrate::import_backdrop(&accepted.values, &document).map_err(|failed| {
            anyhow::anyhow!(
                "backdrop document cannot import: {}",
                serde_json::to_string(&failed).unwrap_or_default()
            )
        })?;
    let operation_id = format!("{IMPORT}-{}", &settings::source_digest(&text)[..16]);
    // Interrupted after the write: its receipt stands, the rename is left.
    if let Some(receipt) = accepted
        .receipts
        .iter()
        .find(|r| r.operation_id == operation_id)
    {
        let receipt = receipt.clone();
        drop(store);
        std::fs::rename(path, &done)?;
        sync_parent(&done)?;
        return Ok(
            json!({"status":"imported","receipt":receipt,"replayed":true,
            "report":imported.report,"document":done}),
        );
    }
    let request = ApplyRequest {
        binding: binding.clone(),
        expected_incarnation: accepted.incarnation.clone(),
        expected_revision: accepted.revision,
        operation_id,
        changes: wallpaper(&imported.values),
        reset: vec!["wallpaper.*".into()],
        request_digest: None,
    };
    let mut authority = crate::authority::Authority::new(store, accepted)?;
    let reply = authority
        .apply(request)
        .map_err(|refused| anyhow::anyhow!("backdrop import refused: {refused}"))?;
    std::fs::rename(path, &done)?;
    sync_parent(&done)?;
    Ok(
        json!({"status":"imported","receipt":reply["receipt"],"replayed":reply["replayed"],
        "report":imported.report,"document":done}),
    )
}

fn sync_parent(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        // A bare file name has the empty parent: the working directory.
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn migrated_name(path: &Path) -> anyhow::Result<std::path::PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("backdrop document needs a file name"))?;
    let mut done = name.to_owned();
    done.push(".migrated");
    Ok(path.with_file_name(done))
}

/// The parsed document, its whole numbers integers again (strict data has
/// one number type), and its text.
fn read_backdrop(path: &Path) -> anyhow::Result<(Value, String)> {
    let text =
        std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let document: Value =
        strict::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok((integral(document), text))
}

fn wallpaper(values: &registry::Values) -> std::collections::BTreeMap<String, Value> {
    values
        .iter()
        .filter(|(path, _)| path.starts_with("wallpaper."))
        .map(|(path, value)| (path.clone(), value.clone()))
        .collect()
}
