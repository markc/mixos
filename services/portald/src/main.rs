// SPDX-License-Identifier: MIT OR Apache-2.0
use clap::{Parser, Subcommand};
use settings::Binding;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "portald",
    version,
    about = "XDG desktop portal on the session bus: the Settings interface"
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
    } = Cli::parse().command;
    let binding = Binding { instance, profile };
    binding
        .validate()
        .map_err(|diagnostic| anyhow::anyhow!(diagnostic.message))?;
    let address = portald::env::session_address()?;
    let state_dir = state_dir.or_else(portald::env::state_directory);
    tokio::runtime::Builder::new_multi_thread()
        // This I/O daemon needs a small fixed runtime; creating one worker per
        // CPU would make process startup depend on the host's core count.
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(portald::run(binding, address, state_dir))
}
