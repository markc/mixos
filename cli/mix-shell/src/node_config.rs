// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Broker discovery delegates to libs/bus and its shared node-config loader.
//! The resident captures environment inputs on the main thread, then resolves
//! URL, socket candidates and account together on its worker.

pub(crate) use ::bus::client_helpers::BrokerEnvironment as NativeEnvironment;

pub fn resolve_noded_url() -> String {
    ::bus::noded_url()
}
