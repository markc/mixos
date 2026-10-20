// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Local semantic indexing engine. The transport is added separately.
//! Uses the system allocator so idle model unload can return pages to the OS.

pub mod bridge;
pub mod bus;
pub mod config;
mod index_file;
mod limits;
mod metrics;
mod model;
mod search;
mod service;
mod store;
#[cfg(test)]
mod test_support;
mod vindex;
mod wire;

pub use service::Engine;

use anyhow::{Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::nomic_bert::{self, NomicBertModel};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokenizers::Tokenizer;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use config::*;
use index_file::*;
use limits::*;
use metrics::{METRICS, Outcome, ReqTiming, RequestContext, classify_response};
use model::*;
use search::*;
use service::*;
use store::*;
use wire::*;
