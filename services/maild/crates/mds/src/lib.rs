// SPDX-License-Identifier: MIT OR Apache-2.0
//! Metadata store: per-container-set SQLite metadata and content-addressable blobs.
//! See the `Mds` trait for the storage contract and the crate README for operation.

pub mod error;
pub mod types;

pub mod blob;
pub mod blob_index;
pub mod bus;
pub mod container;
pub mod export;
pub mod gc;
pub mod import;
pub mod notifier;
pub mod rebuild;
pub mod schema;
pub mod store;
pub mod verify;

#[cfg(any(test, feature = "mem-store"))]
pub mod mem;

pub use crate::error::{Error, Result};
pub use crate::store::{Mds, SqliteCasMds, SqliteSetTx};
pub use crate::types::*;

#[cfg(any(test, feature = "mem-store"))]
pub use crate::mem::MemMds;
