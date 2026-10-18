// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Opt-in `<svc>.log` namespace and live filter watcher.
//!
//! Call [`register`] with the service router and SQLite store, then
//! [`attach`] with the logging handle after the Tokio runtime starts.

mod namespace;
mod watch;

pub use namespace::{
    CANONICAL_KEY, INVALID_FILTER_PREFIX, LogNamespace, LogNamespaceHooks, NAMESPACE,
    UNDELETABLE_PREFIX, UNKNOWN_ENUM_VARIANT_PREFIX, applied_by_variants, auth_policy,
    format_variants, level_variants, namespace_name, register, schema, spec,
};
pub use watch::attach;
