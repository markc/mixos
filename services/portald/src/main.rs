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
        /// Frame-rate ceiling offered to ScreenCast consumers (1..=120).
        #[arg(long, default_value = "30")]
        screencast_max_framerate: portald::screencast::MaxFramerate,
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
        screencast_max_framerate,
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
    let result = runtime.block_on(portald::run(binding, address, state_dir, screencast.then_some(screencast_consent), screencast_max_framerate));
    // A blocking task stuck on a wedged compositor (ScreenCast) must not hold
    // up exit: give the rest a moment, then leave.
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screencast_framerate_default_and_range() {
        let Command::Serve { screencast_max_framerate, .. } =
            Cli::try_parse_from(["portald", "serve", "--instance", "test"]).unwrap().command;
        assert_eq!(screencast_max_framerate.get(), 30);
        for rate in ["1", "60", "120"] {
            assert!(Cli::try_parse_from(["portald", "serve", "--instance", "test", "--screencast-max-framerate", rate]).is_ok());
        }
        for rate in ["0", "121", "-1", "invalid"] {
            let error = Cli::try_parse_from(["portald", "serve", "--instance", "test", "--screencast-max-framerate", rate]).err().unwrap().to_string();
            assert!(error.contains("screencast-max-framerate"), "{error}");
        }
        assert!("0".parse::<portald::screencast::MaxFramerate>().unwrap_err().contains("SCREENCAST_MAX_FRAMERATE"));
    }
}
