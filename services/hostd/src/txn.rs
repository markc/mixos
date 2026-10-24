// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The transaction every write command runs through, whatever it changes:
//! a receipt recorded before anything moves, the change, its proof, then
//! keep or put back; recovery of changes a stop or crash cut short; undo.
//!
//! A command supplies the files it writes and how to make the host use them
//! (`reapply`). Everything about ordering, durability, the circuit breaker
//! and blocked writes lives here, once.
use crate::{
    link,
    receipt::{self, Holds, Outcome, Receipt},
    service::Host,
    wg, wifi,
};
use anyhow::Context;
use serde_json::{Value, json};

fn internal(e: anyhow::Error) -> Value {
    json!({"status":"internal","message":format!("{e:#}")})
}

/// Records a new receipt before anything moves. The save can fail after
/// its rename made the receipt durable (a failed directory sync), so any
/// failure blocks writes until recovery has accounted for it.
pub fn begin(host: &Host, receipt: &Receipt) -> Result<(), Value> {
    receipt.save(&host.state).map_err(|e| {
        host.block(format!(
            "receipt {} may be half-recorded: {e:#}",
            receipt.id
        ));
        internal(e)
    })
}

/// Ends a change: keeps it (`passed`) or puts it back, counts it toward the
/// grant's circuit breaker, and records the outcome. Returns whether the
/// grant is now suspended.
///
/// Breaker order, so a crash at any point errs toward suspending:
/// - a failure is counted before the receipt leaves `applying` (as in
///   recovery): a crash in between leaves an in-flight receipt that
///   recovery counts again, twice at worst, never not at all;
/// - a success resets the streak only after `applied` is durable: a crash
///   before that leaves the streak as it was, and recovery then puts the
///   change back and counts it.
pub async fn conclude(
    host: &Host,
    receipt: &mut Receipt,
    passed: bool,
    reason: Option<String>,
) -> Result<bool, Value> {
    if passed {
        receipt.outcome = Outcome::Applied;
    } else {
        // If putting back fails, the receipt stays `applying` and writes
        // stop until a retry finishes it.
        match put_back(host, receipt).await {
            Ok(Put::Restored) => {
                receipt.outcome = Outcome::RolledBack;
                receipt.reason = reason;
            }
            Ok(Put::LeftAlone) => {
                receipt.outcome = Outcome::Abandoned;
                receipt.reason = Some(format!(
                    "{}; a file changed underneath, left alone",
                    reason.unwrap_or_default()
                ));
            }
            Err(e) => {
                host.block(format!("rollback of {} failed: {e:#}", receipt.id));
                return Err(json!({"status":"rollback_failed","receipt":receipt.id,
                    "message":format!("{e:#}")}));
            }
        }
    }
    let mut suspended = false;
    if !passed {
        match receipt::record(&host.state, &receipt.grant, true) {
            Ok(tripped) => suspended = tripped,
            Err(e) => {
                host.block(format!(
                    "breaker for {} could not be recorded: {e:#}",
                    receipt.id
                ));
                return Err(internal(e));
            }
        }
    }
    if let Err(e) = receipt.save(&host.state) {
        // The durable receipt still says `applying`. Block writes so nothing
        // builds on a change hostd can't account for; the retry, or the next
        // start, finishes it from what the files hold.
        host.block(format!(
            "receipt {} could not be recorded: {e:#}",
            receipt.id
        ));
        return Err(internal(e));
    }
    if passed && let Err(e) = receipt::record(&host.state, &receipt.grant, false) {
        // The streak stays as it was: stricter, never looser. Not a reason
        // to block writes.
        tracing::warn!(error = %e, grant = %receipt.grant, "breaker reset not recorded");
    }
    Ok(suspended)
}

/// What `put_back` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Put {
    /// Every file now holds its backup, and the host was told.
    Restored,
    /// A file held neither hostd's content nor the backup: someone else's
    /// change, which hostd never overwrites.
    LeftAlone,
}

/// Puts a receipt's files back to their backups and makes the host use
/// them. Idempotent, so a failed attempt can simply run again:
///
/// - a file still holding what this change installed is restored;
/// - a file already holding the backup is left as it is;
/// - if any file holds anything else, nothing is touched (`LeftAlone`).
///
/// The command's `reapply` runs whenever the files hold their backups, even if
/// this call wrote nothing, because an earlier attempt may have written the
/// files and then failed before the host was told.
pub async fn put_back(host: &Host, receipt: &Receipt) -> anyhow::Result<Put> {
    for change in &receipt.files {
        if change.holds()? == Holds::Other {
            return Ok(Put::LeftAlone);
        }
    }
    // The live object a command would reconfigure or delete must still be
    // hostd's: a WireGuard link replaced by someone else's is left alone.
    if receipt.command == wg::COMMAND && !wg::owns(receipt).await? {
        return Ok(Put::LeftAlone);
    }
    // A Wi-Fi station someone has since moved to another network is theirs.
    if receipt.command == wifi::COMMAND && !wifi::owns(receipt).await? {
        return Ok(Put::LeftAlone);
    }
    for change in &receipt.files {
        if change.holds()? == Holds::Installed {
            receipt::restore(std::slice::from_ref(change))?;
        }
    }
    match receipt.command.as_str() {
        link::COMMAND => link::reapply(host, receipt).await?,
        wg::COMMAND => wg::reapply(host, receipt).await?,
        wifi::COMMAND => wifi::reapply(host, receipt).await?,
        other => anyhow::bail!(
            "receipt {} is for {other}, which this hostd can't put back",
            receipt.id
        ),
    }
    Ok(Put::Restored)
}

/// Finishes one in-flight receipt and records how it ended. Leaves it in
/// flight, and returns the error, when it can't be finished yet.
async fn finish(host: &Host, receipt: &mut Receipt) -> anyhow::Result<()> {
    let put = put_back(host, receipt).await?;
    let was = receipt.outcome.clone();
    receipt.outcome = match (put, &was) {
        (Put::LeftAlone, _) => Outcome::Abandoned,
        (Put::Restored, Outcome::Undoing) => Outcome::Undone,
        (Put::Restored, _) => Outcome::RolledBack,
    };
    receipt.reason = Some(match (&receipt.outcome, &was) {
        (Outcome::Abandoned, _) => "interrupted; a file changed underneath, left alone".into(),
        (Outcome::Undone, _) => "undo finished".into(),
        _ => "interrupted; rolled back".into(),
    });
    // An apply that never proved healthy is a failed change, however it
    // ended: it counts toward the grant's circuit breaker. Counted before
    // the receipt is saved, so a save that fails and is retried may count
    // it twice, but never not at all.
    if was == Outcome::Applying {
        receipt::record(&host.state, &receipt.grant, true)?;
    }
    receipt.save(&host.state)?;
    // A finished undo is routine; recovering an interrupted change is not.
    if was == Outcome::Undoing && receipt.outcome == Outcome::Undone {
        tracing::info!(target: "hostd::audit", receipt = %receipt.id, outcome = ?receipt.outcome, "finished");
    } else {
        tracing::warn!(target: "hostd::audit", receipt = %receipt.id, outcome = ?receipt.outcome, "finished");
    }
    Ok(())
}

/// `hostd.undo`. An applied receipt must still own its files: if a later
/// change or someone's edit replaced one, undo is refused (`superseded`),
/// because restoring would throw that away. The receipt is marked `undoing`
/// before anything moves, so an undo that fails part way is finished by a
/// retry or by the next start.
pub async fn undo(host: &Host, receipt: &mut Receipt) -> Result<(), Value> {
    if receipt.outcome == Outcome::Applied {
        for change in &receipt.files {
            if change.holds().map_err(internal)? != Holds::Installed {
                return Err(json!({"status":"superseded","receipt":receipt.id,
                    "file": change.path}));
            }
        }
        // Wi-Fi writes no files: its change is current only while the
        // station is still on the network it connected.
        if receipt.command == wifi::COMMAND
            && !wifi::current(host, receipt).await.map_err(internal)?
        {
            return Err(json!({"status":"superseded","receipt":receipt.id,
                "message":"the station is no longer on this network"}));
        }
        receipt.outcome = Outcome::Undoing;
        // From here a durable `undoing` receipt may exist, whatever fails:
        // nothing else may change the host until it is finished.
        if let Err(e) = receipt.save(&host.state) {
            host.block(format!(
                "undo of {} could not be recorded: {e:#}",
                receipt.id
            ));
            return Err(internal(e));
        }
    }
    if let Err(e) = finish(host, receipt).await {
        host.block(format!("undo of {} incomplete: {e:#}", receipt.id));
        return Err(json!({"status":"undo_incomplete","receipt":receipt.id,
            "message":format!("{e:#}")}));
    }
    if receipt.outcome != Outcome::Undone {
        return Err(json!({"status":"superseded","receipt":receipt.id,"outcome":receipt.outcome}));
    }
    Ok(())
}

/// Finishes every in-flight receipt, oldest first. Stops at the first that
/// can't be finished, and returns that error: the caller keeps writes
/// blocked until a later call succeeds.
pub async fn recover(host: &Host) -> anyhow::Result<Vec<String>> {
    let mut done = vec![];
    for mut receipt in receipt::unfinished(&host.state)? {
        finish(host, &mut receipt)
            .await
            .with_context(|| format!("finish receipt {}", receipt.id))?;
        done.push(receipt.id.clone());
    }
    Ok(done)
}
