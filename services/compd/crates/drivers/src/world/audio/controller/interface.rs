// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
pub mod interface;
#[cfg(feature = "desktop-dbus")]
pub mod media;
#[cfg(not(feature = "desktop-dbus"))]
#[path = "interface/media_unavailable.rs"]
pub mod media;
