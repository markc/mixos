// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `settingsd downgrade --to 4` (design §4.4; theming ADR §5): format 5
//! back to a format-4 record the previous settingsd serves, data first and
//! then the binary. With settingsd stopped:
//!
//! 1. The candidate: the reverse conversion of the current values, so
//!    changes since the migration survive, refused naming the blocking
//!    keys when any value has no format-4 home; or, with `--from-kept`,
//!    the kept record `desktop.format4.conf.mix` byte for byte, losing the
//!    changes since, which the report lists for a manual reapply. The kept
//!    route also serves when the format-5 primary is damaged or gone: the
//!    damaged one is linked aside as `corrupt-<uuid>.conf.mix` and `lost` is
//!    `unavailable`.
//! 2. Proved with the target: staged as primary, backup and writer lock in
//!    a private directory, where the target binary's own `seed --root` must accept it
//!    before either live file is touched.
//! 3. The format-5 files are kept as `desktop.format5.conf.mix` and
//!    `desktop.format5.previous.conf.mix`, then the backup and the primary
//!    are replaced by the candidate.
//!
//! Each step is skipped when its file already holds its output, so an
//! interrupted downgrade finishes when run again; once the primary is
//! format 4, the kept format-5 record is the source.
use crate::format4::{self, Record};
use crate::migration::KEPT;
use crate::store::{self, Accepted, BACKUP, PRIMARY, Store, Stored, encode};
use serde_json::{Value, json};
use settings::*;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

pub const KEPT5: &str = "desktop.format5.conf.mix";
pub const KEPT5_PREVIOUS: &str = "desktop.format5.previous.conf.mix";
/// The receipt the reverse conversion leaves in the format-4 record.
pub const RECEIPT: &str = "downgrade-4";
const STAGE: &str = "downgrade-stage";

/// The target settingsd's own check of a staged candidate: `seed --root`.
pub fn seed_with(target: &Path) -> impl Fn(&Path, &Binding) -> anyhow::Result<()> + '_ {
    move |stage, binding| {
        let output = std::process::Command::new(target)
            .arg("seed")
            .arg("--root")
            .arg(stage)
            .args([
                "--instance",
                &binding.instance,
                "--profile",
                &binding.profile,
            ])
            .output()
            .map_err(|e| anyhow::anyhow!("{}: {e}", target.display()))?;
        anyhow::ensure!(
            output.status.success(),
            "{} seed refused the candidate: {}",
            target.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(())
    }
}

/// Downgrade the profile at `root` to format `to` (only 4 exists), proving
/// the candidate with `prove` (in production [`seed_with`] the target).
pub fn downgrade(
    root: &Path,
    binding: &Binding,
    to: u32,
    from_kept: bool,
    prove: impl Fn(&Path, &Binding) -> anyhow::Result<()>,
) -> anyhow::Result<Value> {
    anyhow::ensure!(
        to == format4::FORMAT,
        "settingsd downgrades only to format 4"
    );
    let store = Store::lock(root, false)?;
    let source = format5_source(&store, binding);
    // The format-5 text to keep, None when it is already kept (a resumed run)
    // or there is none to keep.
    let (candidate, mut report, keep) = if from_kept {
        // The kept record is for when the live files are damaged too: a
        // format-5 record that does not read only loses the report of what
        // changed since the migration.
        let source = source
            .map_err(|e| eprintln!("settingsd: no format-5 record to compare ({e})"))
            .ok();
        let (candidate, report) = from_kept_record(
            &store,
            binding,
            source.as_ref().map(|(accepted, ..)| accepted),
        )?;
        let keep = source.and_then(|(_, text, resumed)| (!resumed).then_some(text));
        (candidate, report, keep)
    } else {
        let (accepted, text, resumed) = source?;
        let (candidate, report) = reverse(&accepted, &text)?;
        (candidate, report, (!resumed).then_some(text))
    };
    // Prove it with the target before either live file is touched.
    let stage = store.root().join(STAGE);
    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }
    std::fs::DirBuilder::new().mode(0o700).create(&stage)?;
    // Primary, backup and an established writer lock: a profile, so even a
    // target that would create one where no lock exists only opens it.
    let staged = (|| {
        for name in [PRIMARY, BACKUP] {
            write_private(&stage.join(name), &candidate)?;
        }
        write_private(&stage.join(store::LOCK), b"")?;
        prove(&stage, binding)
    })();
    std::fs::remove_dir_all(&stage)?;
    staged?;
    // Keep format 5, then replace backup and primary; each skipped when done.
    match keep {
        Some(text) => store.keep(KEPT5, text.as_bytes())?,
        // A primary that is not a readable format-5 record, nor yet the
        // candidate, is evidence: keep its inode before it is replaced.
        None => {
            if let Ok(bytes) = store.read(PRIMARY)
                && bytes != candidate
                && !matches!(
                    std::str::from_utf8(&bytes).map(Stored::parse),
                    Ok(Ok(Stored::Current(_)))
                )
            {
                store.link(
                    PRIMARY,
                    &format!("corrupt-{}.conf.mix", uuid::Uuid::now_v7()),
                )?;
            }
        }
    }
    if let Ok(backup) = store.read_text(BACKUP)
        && matches!(Stored::parse(&backup), Ok(Stored::Current(_)))
    {
        store.keep(KEPT5_PREVIOUS, backup.as_bytes())?;
    }
    let mut changed = false;
    for name in [BACKUP, PRIMARY] {
        if store.read(name).ok().as_deref() != Some(candidate.as_slice()) {
            store.replace(name, &candidate)?;
            changed = true;
        }
    }
    report["status"] = if changed { "downgraded" } else { "unchanged" }.into();
    report["to"] = to.into();
    report["kept"] = json!([KEPT5, KEPT5_PREVIOUS]);
    Ok(report)
}

/// The format-5 record to downgrade, its text, and whether this is a resumed
/// run: the primary, or, once the primary is the candidate, the record the
/// downgrade kept.
fn format5_source(store: &Store, binding: &Binding) -> anyhow::Result<(Accepted, String, bool)> {
    let primary = store.read_text(PRIMARY)?;
    let (source, text, resumed) = match Stored::parse(&primary)? {
        Stored::Current(accepted) => (accepted, primary, false),
        Stored::Format4(_) => {
            let kept = store.read_text(KEPT5).map_err(|e| {
                anyhow::anyhow!("{PRIMARY} is already format 4 and {KEPT5} is unreadable ({e})")
            })?;
            match Stored::parse(&kept)? {
                Stored::Current(accepted) => (accepted, kept, true),
                Stored::Format4(_) => anyhow::bail!("{KEPT5} is not a format-5 record"),
            }
        }
    };
    source.check(binding)?;
    Ok((source, text, resumed))
}

/// The reverse conversion of `source`: refused while any value has no
/// format-4 home. Revisions advance by one and a [`RECEIPT`] receipt
/// records the step, so fenced writers see a normal next revision.
fn reverse(source: &Accepted, text: &str) -> anyhow::Result<(Vec<u8>, Value)> {
    let reversed = settings::migrate::reverse(&source.values);
    anyhow::ensure!(
        reversed.blocking.is_empty(),
        "format 4 cannot hold {}; set them back to their defaults, or downgrade --from-kept (losing every change since the migration)",
        reversed.blocking.join(", ")
    );
    let next = |revision: Revision| {
        revision
            .0
            .checked_add(1)
            .map(Revision)
            .ok_or_else(|| anyhow::anyhow!("accepted revision exhausted"))
    };
    let revision = next(source.revision)?;
    let mut receipts: Vec<Receipt> = source
        .receipts
        .iter()
        .filter(|r| r.operation_id != RECEIPT)
        .cloned()
        .collect();
    receipts.push(Receipt {
        operation_id: RECEIPT.into(),
        request_digest: settings::source_digest(text),
        incarnation: source.incarnation.clone(),
        revision,
        outcome: Outcome::Changed,
    });
    if receipts.len() > MAX_RECEIPTS {
        receipts.remove(0);
    }
    let effective = v1::resolve_with_embedded(&reversed.desktop, &source.embedded_source)
        .map_err(|e| anyhow::anyhow!("the format-4 record does not resolve: {e:?}"))?;
    let mut record = Record {
        schema: SCHEMA,
        format: format4::FORMAT,
        binding: source.binding.clone(),
        incarnation: source.incarnation.clone(),
        revision,
        design_revision: next(source.design_revision)?,
        desktop: reversed.desktop,
        embedded_source: source.embedded_source.clone(),
        effective_digest: settings::digest(&effective)?,
        receipts,
        content_digest: String::new(),
    };
    record.seal()?;
    record.check(&source.binding)?;
    record.effective()?;
    Ok((
        encode(&record)?,
        json!({"route":"reverse","revision":record.revision}),
    ))
}

/// The kept format-4 record, unchanged, and the values set since the
/// migration that it does not have.
fn from_kept_record(
    store: &Store,
    binding: &Binding,
    source: Option<&Accepted>,
) -> anyhow::Result<(Vec<u8>, Value)> {
    let text = store
        .read_text(KEPT)
        .map_err(|e| anyhow::anyhow!("--from-kept needs {KEPT}: {e}"))?;
    let Stored::Format4(kept) = Stored::parse(&text)? else {
        anyhow::bail!("{KEPT} is not a format-4 record");
    };
    kept.check(binding)
        .map_err(|e| anyhow::anyhow!("{KEPT}: {e}"))?;
    let revision = kept.revision;
    let (upgraded, _) = (*kept).upgrade()?;
    // What changed since the migration, when both sides read: otherwise
    // the report says it is unavailable rather than guessing.
    let then = settings::migrate::migrate(&upgraded.desktop)
        .map(|m| store::canonical(m.values))
        .ok();
    let lost = match (then, source) {
        (Some(then), Some(source)) => Value::Array(
            then.keys()
                .chain(source.values.keys())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .filter(|path| then.get(*path) != source.values.get(*path))
                .map(|path| {
                    json!({"path":path,"kept":then.get(path),"current":source.values.get(path)})
                })
                .collect(),
        ),
        _ => "unavailable".into(),
    };
    Ok((
        text.into_bytes(),
        json!({"route":"from_kept","revision":revision,"lost":lost}),
    ))
}

fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
