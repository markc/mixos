// SPDX-License-Identifier: MIT OR Apache-2.0
use clap::{Parser, Subcommand};
use hostd::{local, service::Host};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "hostd",
    version,
    about = "Host information and privileged host actions through closed, typed verbs"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the local socket, and the Bus when a broker is reachable.
    Serve {
        /// Used when systemd does not pass a socket.
        #[arg(long, default_value = local::SOCKET)]
        socket: PathBuf,
        /// Serve the local socket only.
        #[arg(long)]
        no_bus: bool,
        /// Read the host, policy and state from this directory instead of
        /// the system paths (tests). The policy need not be root-owned.
        #[arg(long, hide = true)]
        root: Option<PathBuf>,
    },
    /// Send one request to the local socket and print the reply body.
    Call {
        verb: String,
        /// The request body as JSON.
        #[arg(default_value = "{}")]
        body: String,
        #[arg(long, default_value = local::SOCKET)]
        socket: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    buildinfo::exit_on_version!();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        // Under systemd stderr is the journal: no colour codes there.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .try_init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        // A small fixed runtime, independent of the host's core count.
        .worker_threads(2)
        .enable_all()
        .build()?;
    match Cli::parse().command {
        Command::Serve {
            socket,
            no_bus,
            root,
        } => runtime.block_on(hostd::run(hostd::Config {
            socket,
            noded_url: (!no_bus).then(bus::client_helpers::resolve_noded_url),
            host: root.map_or_else(Host::system, |root| Host::fixture(&root)),
        })),
        Command::Call { verb, body, socket } => {
            let body = serde_json::from_str(&body)?;
            let (rc, reply) = runtime.block_on(local::call(&socket, &verb, body))?;
            println!("{}", serde_json::to_string_pretty(&reply)?);
            std::process::exit(if rc == 0 { 0 } else { 1 });
        }
    }
}
