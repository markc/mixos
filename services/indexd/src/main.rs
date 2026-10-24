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
    Bridge {
        /// Node configuration (defaults to the system service's config).
        #[arg(long)]
        node_config: Option<PathBuf>,
    },
    /// Execute one command in-process, without the Bus.
    Call {
        command: String,
        #[arg(default_value = "{}")]
        json_body: String,
        /// Allow writes only when the daemon is stopped.
        #[arg(long)]
        offline: bool,
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
    if let Command::Bridge { node_config } = &cli.command {
        let node_config = node_config
            .clone()
            .or_else(|| {
                std::env::var_os("MIXOS_NODE_CONFIG")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from("/opt/mixos/etc/node.conf.mix"));
        return runtime.block_on(async {
            let client = async {
                let url = match std::env::var("MIXOS_NODED_URL")
                    .ok()
                    .filter(|v| !v.is_empty())
                {
                    Some(url) => url,
                    None => {
                        let source = std::fs::read_to_string(&node_config)?;
                        ::bus::client::url_from_node_config(&source).ok_or_else(|| {
                            anyhow::anyhow!("invalid node config: {}", node_config.display())
                        })?
                    }
                };
                ::bus::native_client::NodedClient::connect_anonymous(&url).await
            }
            .await;
            let result = indexd::bridge::run(
                tokio::io::BufReader::new(tokio::io::stdin()),
                tokio::io::stdout(),
                &client,
            )
            .await;
            if let Ok(client) = client {
                client.close().await;
            }
            result
        });
    }
    if let Command::Call {
        command: command_name,
        json_body,
        offline,
    } = &cli.command
    {
        let body: serde_json::Value = serde_json::from_str(json_body)?;
        anyhow::ensure!(
            body.get("background").and_then(serde_json::Value::as_bool) != Some(true),
            "indexd call refuses background:true: no process remains to run the job; use the daemon"
        );
        let action = command_name.strip_prefix("indexd.").unwrap_or(command_name);
        anyhow::ensure!(
            *offline
                || !matches!(
                    action,
                    "store" | "update" | "delete" | "feedback" | "supersede" | "index_file"
                ),
            "indexd call writes require --offline with the daemon stopped: a running daemon's in-memory index would desynchronise"
        );
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
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
                tokio::select! {
                    _ = &mut service => {},
                    _ = term.recv() => { let _ = stop_tx.send(true); service.await; },
                    _ = tokio::signal::ctrl_c() => { let _ = stop_tx.send(true); service.await; },
                }
                Ok(())
            }
            Command::Call {
                command: command_name,
                json_body,
                ..
            } => {
                let reply = engine
                    .call_sized(
                        &command_name,
                        serde_json::from_str(&json_body)?,
                        json_body.len(),
                    )
                    .await;
                println!("{reply}");
                if reply.get("error").is_some() {
                    anyhow::bail!("indexd call failed");
                }
                Ok(())
            }
            Command::Bridge { .. } | Command::CheckConfig => unreachable!(),
        }
    });
    drop(engine);
    // Running blocking requests finish before the runtime is torn down.
    runtime.shutdown_timeout(std::time::Duration::from_secs(150));
    result
}
