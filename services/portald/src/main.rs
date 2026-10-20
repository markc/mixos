// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use clap::{Parser, Subcommand};
use settings::Binding;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "portald",
    version,
    about = "XDG desktop portal on the session bus: Settings and ScreenCast"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve org.freedesktop.portal.Desktop on the session bus.
    Serve {
        #[arg(long)]
        instance: String,
        #[arg(long, default_value = "default")]
        profile: String,
        /// Holds the validated appearance cache. Defaults to $STATE_DIRECTORY.
        #[arg(long)]
        state_dir: Option<PathBuf>,
        /// Also serve org.freedesktop.portal.ScreenCast (needs a compositor
        /// with zwlr_screencopy_manager_v1 and a PipeWire session).
        #[arg(long)]
        screencast: bool,
        /// How screen-share requests are decided: `prompt` asks the person
        /// (or an agent over the Bus); `allow` grants every request, for
        /// headless and test sessions only.
        #[arg(long, default_value = "prompt")]
        screencast_consent: portald::screencast::consent::Policy,
    },
}

fn main() -> anyhow::Result<()> {
    buildinfo::exit_on_version!();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
    let Command::Serve {
        instance,
        profile,
        state_dir,
        screencast,
        screencast_consent,
    } = Cli::parse().command;
    let binding = Binding { instance, profile };
    binding
        .validate()
        .map_err(|diagnostic| anyhow::anyhow!(diagnostic.message))?;
    let address = portald::env::session_address()?;
    let state_dir = state_dir.or_else(portald::env::state_directory);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        // This I/O daemon needs a small fixed runtime; creating one worker per
        // CPU would make process startup depend on the host's core count.
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(portald::run(binding, address, state_dir, screencast.then_some(screencast_consent)));
    // A blocking task stuck on a wedged compositor (ScreenCast) must not hold
    // up exit: give the rest a moment, then leave.
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}
