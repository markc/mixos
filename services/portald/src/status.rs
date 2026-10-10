// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the portal knows about its own evidence. Served by `portald.status`;
//! never fed back into the portal values.
use serde::Serialize;
use serde_json::{Value, json};
use settings::Revision;
use std::sync::{Arc, PoisonError, RwLock};

/// Where the values now served came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// Embedded defaults; no authority or cache has answered.
    Defaults,
    /// A validated session-local cache; not yet confirmed by settingsd.
    Cache,
    /// A projection accepted from settingsd in this process.
    Settingsd,
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub origin: Origin,
    pub name_owned: bool,
    pub incarnation: Option<String>,
    pub revision: Option<Revision>,
    pub design_revision: Option<Revision>,
    /// The last source fault, cleared by the next good projection.
    pub settings_fault: Option<String>,
    pub accepted: u64,
    pub stale: u64,
    pub rejected: u64,
}

impl Status {
    pub fn new(origin: Origin) -> Self {
        Self {
            origin,
            name_owned: false,
            incarnation: None,
            revision: None,
            design_revision: None,
            settings_fault: None,
            accepted: 0,
            stale: 0,
            rejected: 0,
        }
    }
}

pub type Shared = Arc<RwLock<Status>>;

pub fn shared() -> Shared {
    Arc::new(RwLock::new(Status::new(Origin::Defaults)))
}

pub fn update(shared: &Shared, change: impl FnOnce(&mut Status)) {
    change(&mut shared.write().unwrap_or_else(PoisonError::into_inner));
}

/// The reply to `portald.status`.
pub fn report(shared: &Shared) -> Value {
    let snapshot = shared
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    json!({"status": "current", "portal": snapshot})
}
