// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Shared infrastructure for mixos daemon services.
//!
//! Provides common initialization patterns used by all 6 daemons:
//! the log directory path, graceful shutdown signal, and optional TLS
//! configuration. Tracing/logging init now lives in `logging`
//! (bus repo) + `logging::props` (the `<svc>.log` namespace).

#[cfg(feature = "tls")]
pub mod tls;

// ACME wrapper — bundled into the `tls` feature (see Cargo.toml).
// Phase 2 Commit 2 ships the surface with no in-workspace caller;
// the webd `AcmeProvisioner` in Commit 5 will be the first.
#[cfg(feature = "tls")]
pub mod acme;

// Self-signed leaf minting (rcgen) for internal/`.localhost` vhosts — replaces
// the NS5 firstboot `openssl` shell-out so the appliance stays pure-glibc.
#[cfg(feature = "tls")]
pub mod selfcert;

pub mod http_host;

// Generic per-interface listener-set manager. Compiles under default
// features (plain-TCP path); TLS termination is gated inside the
// module on `tls`. webd is the first consumer; maild migrates later.
pub mod listen;

/// Returns the log directory: `~/.local/log/mixos/`.
pub fn log_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/log/mixos"))
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/mixos-log"))
}

/// Wait for a shutdown signal (Ctrl+C or SIGTERM on Unix).
///
/// Use with `tokio::select!` in the daemon's main loop:
///
/// ```ignore
/// tokio::select! {
///     _ = serve_forever() => {}
///     _ = daemon::shutdown_signal() => {
///         tracing::info!("shutting down");
///     }
/// }
/// ```
pub async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }

    tracing::info!("shutdown signal received");
}

/// Load a TLS server configuration from PEM cert and key files.
///
/// Returns a `tokio_rustls::TlsAcceptor` ready for use in a TCP accept loop.
/// Installs the `ring` crypto provider on first call.
///
/// ```ignore
/// let acceptor = daemon::load_tls_config("cert.pem", "key.pem")?;
/// ```
#[cfg(feature = "tls")]
pub use config::load_tls_config;
