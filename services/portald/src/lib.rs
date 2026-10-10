// SPDX-License-Identifier: MIT OR Apache-2.0
//! The MixOS Settings portal: `org.freedesktop.portal.Desktop` on the session
//! bus, serving the `org.freedesktop.appearance` namespace from settingsd.
pub mod appearance;
pub mod cache;
pub mod env;
pub mod live;
pub mod portal;
pub mod service;
pub mod status;

use settings::Binding;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Serves the portal until SIGTERM or SIGINT. A failing ABP source is logged
/// and never stops the portal: it keeps serving its last-good values.
pub async fn run(
    binding: Binding,
    address: String,
    state_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    let (projections, received) = mpsc::channel(8);
    let status = status::shared();
    let noded_url = bus::client_helpers::resolve_noded_url();
    let feed_status = status.clone();
    let feed_binding = binding.clone();
    let feed = tokio::spawn(async move {
        if let Err(error) = service::feed(feed_binding, noded_url, projections, feed_status).await {
            tracing::warn!(%error, "settings source unavailable; serving last-good values");
        }
    });
    let result = live::serve(
        live::Config {
            address,
            state_dir,
            binding,
        },
        received,
        status,
    )
    .await;
    feed.abort();
    let _ = feed.await;
    result
}
