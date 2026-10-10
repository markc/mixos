// SPDX-License-Identifier: MIT OR Apache-2.0
//! The environment portald is started with. The unit passes the session bus
//! address through `EnvironmentFile=`; portald keeps it.
use anyhow::Context;
use std::path::PathBuf;

/// The session bus address, required and non-empty.
pub fn session_address() -> anyhow::Result<String> {
    let address =
        std::env::var("DBUS_SESSION_BUS_ADDRESS").context("DBUS_SESSION_BUS_ADDRESS is not set")?;
    anyhow::ensure!(
        !address.trim().is_empty(),
        "DBUS_SESSION_BUS_ADDRESS is empty"
    );
    Ok(address)
}

/// The first directory systemd provides through `StateDirectory=`.
pub fn state_directory() -> Option<PathBuf> {
    std::env::var("STATE_DIRECTORY")
        .ok()?
        .split(':')
        .find(|part| !part.is_empty())
        .map(PathBuf::from)
}
