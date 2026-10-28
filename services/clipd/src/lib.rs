// SPDX-License-Identifier: MIT OR Apache-2.0
//! Per-session clipboard ownership, history and explicit Bus transfers.
pub mod menu;
pub mod model;
pub mod service;
pub mod wayland;

/// Shared argument contract for the standalone entry point and future mixosd.
#[derive(clap::Parser)]
#[command(version, about = "Session clipboard on the native MixOS Bus")]
pub struct Args {
    #[arg(long, default_value = "vt1")]
    pub session: String,
    #[arg(long)]
    pub settings: Option<std::path::PathBuf>,
    #[arg(long)]
    pub seat: Option<String>,
    #[arg(long)]
    pub bus: Option<String>,
    /// Open the local history picker; the service remains the clipboard owner.
    #[arg(long)]
    pub menu: bool,
}

/// Service entry point. Owns its runtime so folding into mixosd is dispatch
/// wiring, including the history picker's main-thread GUI requirement.
pub fn run(args: Args) -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
    let url = args
        .bus
        .unwrap_or_else(bus::client_helpers::resolve_noded_url);
    if args.menu {
        return menu::run(url);
    }
    let session = model::session(&args.session)?;
    let (settings, key) = service::Settings::load(args.settings.as_deref(), &session)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(service::run(session, url, settings, key, args.seat));
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}
