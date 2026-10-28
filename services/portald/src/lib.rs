// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The MixOS Settings portal: `org.freedesktop.portal.Desktop` on the session
//! bus, serving the `org.freedesktop.appearance` namespace from settingsd and,
//! `org.freedesktop.portal.FileChooser` from files and, with `--screencast`,
//! `org.freedesktop.portal.ScreenCast` from compd.
pub mod appearance;
pub mod cache;
pub mod env;
pub mod filechooser;
pub mod gtk_settings;
pub mod live;
pub mod portal;
pub mod screencast;
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
    screencast: Option<screencast::consent::Policy>,
    screencast_max_framerate: screencast::MaxFramerate,
) -> anyhow::Result<()> {
    // The feed registers its command manifest when it starts. Establish the
    // instance's consent surface before spawning it, matching live::serve's
    // installation even if the feed wins the startup race.
    if let Some(policy) = screencast {
        screencast::consent::install(screencast::consent::Consent::new(policy));
    }
    let (projections, received) = mpsc::channel(8);
    let status = status::shared();
    let noded_url = bus::client_helpers::resolve_noded_url();
    let feed_status = status.clone();
    let feed_binding = binding.clone();
    let (client, incoming) = service::connect(&noded_url)?;
    screencast::consent::set_client(client.clone());
    let chooser = filechooser::Bridge::new(client.clone());
    let (outcomes, chooser_outcomes) = mpsc::channel(64);
    let feed = tokio::spawn(async move {
        if let Err(error) = service::run(feed_binding, client, filechooser::Source { incoming, outcomes }, projections, feed_status).await {
            tracing::warn!(%error, "settings source unavailable; serving last-good values");
        }
    });
    let config_home = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let result = live::serve(
        live::Config {
            address,
            state_dir,
            binding,
            screencast,
            screencast_max_framerate,
            gtk_settings: gtk_settings::Exporter::new(config_home.as_deref(), home.as_deref()),
            filechooser: Some((chooser, chooser_outcomes)),
        },
        received,
        status,
    )
    .await;
    feed.abort();
    let _ = feed.await;
    result
}
