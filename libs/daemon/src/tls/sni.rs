// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Shared SNI selection; the implementation and its tests belong to config.
pub use config::tls::sni::{SniCertResolver, TlsIdentity, pick_identity};
