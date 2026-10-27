// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! monitord: samples the system and its processes, keeps history, and
//! answers read commands on the Bus, so any node's load can be read from
//! anywhere on the mesh (docs/docs/decisions/2026-10-16-monitord-and-monitor.md).
//!
//! The `monitord` binary, and the system binary started as `monitord`,
//! both run [`main`].
pub mod action;
pub mod bus_loop;
pub mod cli;
pub mod connections;
pub mod engine;
pub mod history;
pub mod kernel;
pub mod process;
pub mod sampler;
pub mod service;

use clap::Parser;
use std::process::ExitCode;

// The global allocator belongs to each binary (src/main.rs, or the system
// binary), never to this library.

/// This component's version, for the system binary's `--personalities`.
pub const COMPONENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct ServeConfig {
    pub engine: engine::Config,
    /// `None` runs without the Bus.
    pub noded_url: Option<String>,
}

/// Sample, record and serve until SIGTERM or SIGINT. A Bus failure is
/// logged and never stops sampling.
pub async fn serve(config: ServeConfig) -> anyhow::Result<()> {
    let engine = engine::Engine::new(config.engine);
    let sampling = tokio::spawn(engine::run(engine.clone()));
    let connections = tokio::spawn(connections::run(engine.connections.clone()));
    let bus = config.noded_url.map(|url| {
        let engine = engine.clone();
        tokio::spawn(async move {
            if let Err(error) = bus_loop::serve(engine, url).await {
                tracing::warn!(%error, "monitord is off the Bus; sampling continues");
            }
        })
    });
    tracing::info!(
        interval_ms = engine.interval.as_millis() as u64,
        "monitord sampling"
    );
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    tokio::select! {
        _ = sigterm.recv() => {}
        _ = sigint.recv() => {}
    }
    sampling.abort();
    connections.abort();
    if let Some(bus) = bus {
        bus.abort();
    }
    let e = engine.clone();
    let _ = tokio::task::spawn_blocking(move || e.flush()).await;
    Ok(())
}

fn exit(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("monitord: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// The whole monitord program: the `monitord` binary and the system
/// binary's `monitord` personality are thin calls into this.
pub fn main() -> ExitCode {
    // --version first, before any runtime exists.
    buildinfo::exit_on_version!();
    let cli = cli::Cli::parse();
    match cli.command {
        cli::Command::Serve {
            no_bus,
            interval_ms,
            db,
            no_db,
            minute_days,
            hour_days,
            root,
        } => {
            let log = logging::init(
                &logging::LogOpts::default(),
                &logging::StatsOpts::default(),
                logging::LogDefaults::daemon("monitord").with_stats(false),
            );
            let _log = match log {
                Ok(handle) => handle,
                Err(error) => {
                    eprintln!("monitord: logging: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                // A small fixed runtime, independent of the host's core count.
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => return exit(Err(error.into())),
            };
            let config = ServeConfig {
                engine: engine::Config {
                    root: kernel::Root::new(root),
                    interval: std::time::Duration::from_millis(interval_ms),
                    db: (!no_db).then(|| db.unwrap_or_else(cli::default_db)),
                    retention: history::Retention {
                        minute_days,
                        hour_days,
                    },
                },
                noded_url: (!no_bus).then(bus::client_helpers::resolve_noded_url),
            };
            exit(runtime.block_on(serve(config)))
        }
        cli::Command::Snapshot { json } => exit(cli::snapshot(json)),
        cli::Command::Ps {
            sort,
            reverse,
            limit,
            filter,
            user,
            tree,
            all,
            json,
        } => exit(cli::ps(
            process::Query {
                sort: Some(sort),
                reverse,
                limit: Some(limit),
                filter,
                user,
                tree,
                kernel_threads: all,
            },
            json,
        )),
        cli::Command::History {
            series,
            since,
            max_points,
            db,
            json,
        } => exit(cli::history(series, since, max_points, db, json)),
        cli::Command::Signal { pid, signal } => exit(cli::signal(pid, &signal)),
        cli::Command::Renice { pid, nice } => exit(cli::renice(pid, nice)),
    }
}
