// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("MIXOS_GIT_SHA"), ", built ", env!("MIXOS_BUILD_TIME"), ")"))]
struct Cli {
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open the engine store. Bus registration arrives in stage 4.
    Serve,
    /// Parse configuration without loading weights.
    CheckConfig,
}

fn main() -> Result<()> {
    buildinfo::exit_on_version!();
    let cli = Cli::parse();
    let _logging = logging::init(
        &logging::LogOpts::default(),
        &logging::StatsOpts::default(),
        logging::LogDefaults::daemon("indexd").with_stats(false),
    )?;
    let (path, cfg) = indexd::config::load_indexd_config(cli.config.as_deref())?;
    cfg.validate()?;
    match cli.command {
        Command::CheckConfig => println!("{}", ::config::to_conf_mix_string(&cfg)?),
        Command::Serve => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let _engine = indexd::Engine::open(cfg)?;
                tracing::info!(config = %path.display(), "bus not wired yet (stage 4)");
                Ok::<_, anyhow::Error>(())
            })?;
        }
    }
    Ok(())
}
