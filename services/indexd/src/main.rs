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
    /// Serve the semantic index on the node's Bus.
    Serve,
    /// Parse configuration without loading weights.
    CheckConfig,
    /// Forward NDJSON requests to the local indexd Bus service.
    Bridge,
    /// Execute one verb in-process, without the Bus.
    Call {
        verb: String,
        #[arg(default_value = "{}")]
        json_body: String,
    },
}

fn main() -> Result<()> {
    buildinfo::exit_on_version!();
    let cli = Cli::parse();
    let _logging = logging::init(
        &logging::LogOpts::default(),
        &logging::StatsOpts::default(),
        logging::LogDefaults::daemon("indexd").with_stats(false),
    )?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
    if matches!(cli.command, Command::Bridge) {
        return runtime.block_on(async {
            let client = ::bus::client_helpers::connect_anonymous_default().await;
            let result = indexd::bridge::run(tokio::io::BufReader::new(tokio::io::stdin()), tokio::io::stdout(), &client).await;
            if let Ok(client) = client { client.close().await; }
            result
        });
    }
    // Config discovery, validation and initial SQLite/index loading precede
    // entering the async runtime. Bridge never opens the store or model files.
    let (path, cfg) = indexd::config::load_indexd_config(cli.config.as_deref())?;
    cfg.validate()?;
    if matches!(cli.command, Command::CheckConfig) {
        println!("{}", ::config::to_conf_mix_string(&cfg)?);
        return Ok(());
    }
    let engine = {
        let _entered = runtime.enter();
        std::sync::Arc::new(indexd::Engine::open(cfg)?)
    };
    let result = runtime.block_on(async {
        match cli.command {
            Command::Serve => {
                tracing::info!(config = %path.display(), "serving indexd");
                let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
                let service = indexd::bus::run(engine.clone(), stop_rx);
                tokio::pin!(service);
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
                tokio::select! {
                    _ = &mut service => {},
                    _ = term.recv() => { let _ = stop_tx.send(true); service.await; },
                    _ = tokio::signal::ctrl_c() => { let _ = stop_tx.send(true); service.await; },
                }
                Ok(())
            }
            Command::Call { verb, json_body } => {
                let reply = engine.call(&verb, serde_json::from_str(&json_body)?).await;
                println!("{reply}");
                if reply.get("error").is_some() { anyhow::bail!("indexd call failed"); }
                Ok(())
            }
            Command::Bridge | Command::CheckConfig => unreachable!(),
        }
    });
    drop(engine);
    // Running blocking requests finish before the runtime is torn down.
    runtime.shutdown_timeout(std::time::Duration::from_secs(150));
    result
}
