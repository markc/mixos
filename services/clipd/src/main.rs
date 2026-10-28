// SPDX-License-Identifier: MIT OR Apache-2.0
use clap::Parser;

#[derive(Parser)]
#[command(version, about = "Session clipboard on the native MixOS Bus")]
struct Args {
    #[arg(long, default_value = "vt1")]
    session: String,
    #[arg(long)]
    settings: Option<std::path::PathBuf>,
    #[arg(long)]
    seat: Option<String>,
    #[arg(long)]
    bus: Option<String>,
    /// Open the local history picker; the service remains the clipboard owner.
    #[arg(long)]
    menu: bool,
}

fn main() -> anyhow::Result<()> {
    buildinfo::exit_on_version!();
    let args = Args::parse();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
    let url = args
        .bus
        .unwrap_or_else(bus::client_helpers::resolve_noded_url);
    if args.menu {
        return clipd::menu::run(url);
    }
    let session = clipd::model::session(&args.session)?;
    let (settings, key) = clipd::service::Settings::load(args.settings.as_deref(), &session)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(clipd::service::run(session, url, settings, key, args.seat));
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    result
}
