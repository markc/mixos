// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! hostd: information about the host, and privileged host actions through
//! closed, typed verbs, behind standing grants, receipts and rollback.
pub mod bus_loop;
pub mod caller;
pub mod desk;
pub mod frame;
pub mod freeze;
pub mod info;
pub mod link;
pub mod local;
pub mod neigh;
pub mod net;
pub mod networkd;
pub mod policy;
pub mod receipt;
pub mod service;
pub mod txn;
pub mod varlink;
pub mod wg;
pub mod wgnl;
pub mod wifi;

use service::Host;
use std::path::PathBuf;

pub struct Config {
    pub socket: PathBuf,
    /// `None` runs without the Bus: the local socket only.
    pub noded_url: Option<String>,
    pub host: Host,
}

/// The longest a stop waits for a running transaction (its health window,
/// at most 60 s, plus its rollback). The unit's `TimeoutStopSec` is longer.
const DRAIN: std::time::Duration = std::time::Duration::from_secs(80);

/// Serves until SIGTERM or SIGINT. A Bus failure is logged and never stops
/// the local socket.
///
/// Writes are held until transactions a previous stop or crash cut short
/// are rolled back; reads are served meanwhile. On a stop, hostd stops
/// accepting and waits for a running transaction to finish, rollback
/// included, before it exits.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let listener = local::listener(&config.socket)?;
    let recovering = config.host.writer.clone().lock_owned().await;
    let local = tokio::spawn(local::serve(listener, config.host.clone()));
    // A failure leaves writes blocked; every later write retries it.
    match config.host.recover().await {
        Ok(done) if !done.is_empty() => tracing::warn!(?done, "finished interrupted transactions"),
        Ok(_) => {}
        Err(error) => tracing::error!(%error, "recovery pending; writes refused until it succeeds"),
    }
    drop(recovering);
    let bus = config.noded_url.map(|url| {
        let host = config.host.clone();
        tokio::spawn(async move {
            if let Err(error) = bus_loop::serve(host, url).await {
                tracing::warn!(%error, "hostd is off the Bus; the local socket still serves");
            }
        })
    });
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let local_abort = local.abort_handle();
    let result = tokio::select! {
        r = local => r?,
        _ = sigterm.recv() => Ok(()),
        _ = sigint.recv() => Ok(()),
    };
    // Stop accepting, then let a running transaction finish.
    local_abort.abort();
    if let Some(bus) = bus {
        bus.abort();
    }
    if tokio::time::timeout(DRAIN, config.host.writer.lock())
        .await
        .is_err()
    {
        tracing::error!("a transaction outlived the stop; the next start rolls it back");
    }
    result
}
