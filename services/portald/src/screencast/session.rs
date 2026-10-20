// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `org.freedesktop.portal.Session` and the registry of live ScreenCast
//! sessions. A session's capture stops when its entry leaves the registry:
//! the stream handle is dropped with it.
use crate::screencast::stream;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use zbus::message::Header;
use zbus::object_server::{ObjectServer, SignalEmitter};
use zbus::zvariant::{OwnedObjectPath, Value};
use zbus::{Connection, fdo};

pub const VERSION: u32 = 1;

/// What one session has been told and what it runs.
pub struct State {
    pub owner: String,
    /// `cursor_mode` from `SelectSources`; `None` until sources are selected.
    pub cursor_mode: Option<u32>,
    /// Set once `Start` has been accepted; a session starts at most once.
    pub starting: bool,
    pub cast: Option<stream::Handle>,
}

#[derive(Clone, Default)]
pub struct Sessions(Arc<Mutex<HashMap<String, State>>>);

impl Sessions {
    pub fn with<R>(&self, f: impl FnOnce(&mut HashMap<String, State>) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub fn insert(&self, path: &str, owner: &str) {
        self.with(|map| {
            map.insert(
                path.to_owned(),
                State {
                    owner: owner.to_owned(),
                    cursor_mode: None,
                    starting: false,
                    cast: None,
                },
            )
        });
    }

    /// Removes a session, stopping its capture. The handle is dropped outside
    /// the lock because stopping joins threads.
    pub fn remove(&self, path: &str) -> Option<State> {
        self.with(|map| map.remove(path))
    }

    /// Every session path owned by `owner`.
    pub fn owned_by(&self, owner: &str) -> Vec<String> {
        self.with(|map| {
            map.iter()
                .filter(|(_, state)| state.owner == owner)
                .map(|(path, _)| path.clone())
                .collect()
        })
    }

    pub fn len(&self) -> usize {
        self.with(|map| map.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct Session {
    owner: String,
    sessions: Sessions,
}

impl Session {
    pub fn new(owner: &str, sessions: Sessions) -> Self {
        Self {
            owner: owner.to_owned(),
            sessions,
        }
    }
}

#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl Session {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }

    /// Closes the session from the caller's side: no `Closed` signal.
    #[zbus(name = "Close")]
    async fn close(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<()> {
        let sender = header.sender().map(|name| name.as_str()).unwrap_or("");
        if sender != self.owner {
            return Err(fdo::Error::AccessDenied("not the owner of this session".into()));
        }
        let Some(path) = header.path() else {
            return Err(fdo::Error::Failed("no object path".into()));
        };
        let path = path.to_owned();
        let state = self.sessions.remove(path.as_str());
        tokio::task::spawn_blocking(move || drop(state));
        server.remove::<Session, _>(path).await?;
        tracing::info!(owner = %self.owner, "screencast session closed by its owner");
        Ok(())
    }

    #[zbus(signal, name = "Closed")]
    async fn closed(emitter: &SignalEmitter<'_>, details: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
}

/// Ends a session from the portal's side (owner gone, capture lost) and
/// removes its object. `notify` emits `Closed` to the owner first.
pub async fn end(connection: &Connection, sessions: &Sessions, path: &str, notify: bool) {
    let Some(state) = sessions.remove(path) else {
        return;
    };
    let owner = state.owner.clone();
    tokio::task::spawn_blocking(move || drop(state));
    let Ok(path) = OwnedObjectPath::try_from(path.to_owned()) else {
        return;
    };
    if notify
        && let Ok(emitter) = SignalEmitter::new(connection, path.clone())
        && let Ok(destination) = zbus::names::BusName::try_from(owner)
    {
        let emitter = emitter.set_destination(destination);
        if let Err(error) = Session::closed(&emitter, HashMap::new()).await {
            tracing::warn!(%error, %path, "Closed not emitted");
        }
    }
    let _ = connection.object_server().remove::<Session, _>(path).await;
}
