// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The `org.freedesktop.portal.ScreenCast` interface (version 5): monitor
//! sources from compd's screencopy, delivered as PipeWire video nodes.
//!
//! Scope of this first cut, deliberately narrow:
//! - **Monitor sources only**, one stream per session: the first output with a
//!   current mode. `multiple` is accepted and ignored.
//! - **No consent dialog.** The session's callers are trusted and
//!   unsandboxed; every grant is logged with its caller and output.
//! - **No restore tokens**: `persist_mode` is accepted and ignored.
//! - **shm frames**, copied twice (screencopy buffer, PipeWire buffer).
pub mod capture;
pub mod handle;
pub mod remote;
pub mod request;
pub mod session;
pub mod stream;

use crate::portal::PortalError;
use session::{Session, Sessions};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use zbus::Connection;
use zbus::message::Header;
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};

pub const INTERFACE: &str = "org.freedesktop.portal.ScreenCast";
pub const VERSION: u32 = 5;
pub const SOURCE_MONITOR: u32 = 1;
pub const CURSOR_HIDDEN: u32 = 1;
pub const CURSOR_EMBEDDED: u32 = 2;
/// The frame-rate ceiling offered to consumers.
pub const MAX_FRAMERATE: u32 = 30;

pub struct ScreenCast {
    sessions: Sessions,
    counter: Arc<AtomicU64>,
}

impl ScreenCast {
    pub fn new(sessions: Sessions) -> Self {
        Self {
            sessions,
            counter: Arc::new(AtomicU64::new(0)),
        }
    }
}

fn sender(header: &Header<'_>) -> Result<String, PortalError> {
    header
        .sender()
        .map(|name| name.as_str().to_owned())
        .ok_or_else(|| PortalError::InvalidArgument("no sender".into()))
}

fn option_u32(options: &HashMap<String, OwnedValue>, key: &str) -> Result<Option<u32>, PortalError> {
    options
        .get(key)
        .map(|value| {
            value
                .downcast_ref::<u32>()
                .map_err(|_| PortalError::InvalidArgument(format!("{key} must be a u32")))
        })
        .transpose()
}

/// The caller's own session at `path`, checked under the registry lock.
fn check_session(
    sessions: &Sessions,
    path: &OwnedObjectPath,
    owner: &str,
    f: impl FnOnce(&mut session::State) -> Result<(), PortalError>,
) -> Result<(), PortalError> {
    if !handle::owned_by(path.as_str(), owner) {
        return Err(PortalError::NotFound(format!("{path} is not your session")));
    }
    sessions.with(|map| match map.get_mut(path.as_str()) {
        Some(state) if state.owner == owner => f(state),
        _ => Err(PortalError::NotFound(format!("no session {path}"))),
    })
}

fn respond_later(
    connection: &Connection,
    path: OwnedObjectPath,
    owner: String,
    code: u32,
    results: HashMap<&'static str, Value<'static>>,
) {
    let connection = connection.clone();
    tokio::spawn(async move {
        request::respond(&connection, &path, &owner, code, results).await;
    });
}

#[zbus::interface(name = "org.freedesktop.portal.ScreenCast")]
impl ScreenCast {
    #[zbus(property, name = "AvailableSourceTypes")]
    fn available_source_types(&self) -> u32 {
        SOURCE_MONITOR
    }

    #[zbus(property, name = "AvailableCursorModes")]
    fn available_cursor_modes(&self) -> u32 {
        CURSOR_HIDDEN | CURSOR_EMBEDDED
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }

    #[zbus(name = "CreateSession")]
    async fn create_session(
        &self,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<OwnedObjectPath, PortalError> {
        let owner = sender(&header)?;
        let request = handle::request_path(&owner, &handle::token(&options, "handle_token", &self.counter)?)?;
        let session = handle::session_path(
            &owner,
            &handle::token(&options, "session_handle_token", &self.counter)?,
        )?;
        if !connection
            .object_server()
            .at(session.clone(), Session::new(&owner, self.sessions.clone()))
            .await?
        {
            return Err(PortalError::InvalidArgument(format!("session {session} already exists")));
        }
        self.sessions.insert(session.as_str(), &owner);
        if let Err(error) = request::export(connection, &request, &owner).await {
            session::end(connection, &self.sessions, session.as_str(), false).await;
            return Err(error.into());
        }
        // The owner may have left before this call was dispatched, in which
        // case its NameOwnerChanged already went by: nothing would retire
        // the session.
        let present = zbus::fdo::DBusProxy::new(connection)
            .await?
            .name_has_owner(header.sender().map(|name| name.clone().into()).ok_or_else(|| {
                PortalError::InvalidArgument("no sender".into())
            })?)
            .await;
        // Only a definite answer ends the session; a failed query keeps it
        // (NameOwnerChanged still retires it if the owner really left).
        if matches!(present, Ok(false)) {
            session::end(connection, &self.sessions, session.as_str(), false).await;
            let _ = connection
                .object_server()
                .remove::<request::Request, _>(request.clone())
                .await;
            return Err(PortalError::InvalidArgument(format!("{owner} left the bus")));
        }
        tracing::info!(%owner, %session, "screencast session created");
        let mut results = HashMap::new();
        results.insert("session_handle", Value::from(session.as_str().to_owned()));
        respond_later(connection, request.clone(), owner, request::SUCCESS, results);
        Ok(request)
    }

    #[zbus(name = "SelectSources")]
    async fn select_sources(
        &self,
        session_handle: OwnedObjectPath,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<OwnedObjectPath, PortalError> {
        let owner = sender(&header)?;
        let request = handle::request_path(&owner, &handle::token(&options, "handle_token", &self.counter)?)?;
        let types = option_u32(&options, "types")?.unwrap_or(SOURCE_MONITOR);
        if types & SOURCE_MONITOR == 0 {
            return Err(PortalError::InvalidArgument(
                "only monitor sources are offered".into(),
            ));
        }
        let cursor_mode = option_u32(&options, "cursor_mode")?.unwrap_or(CURSOR_HIDDEN);
        if cursor_mode != CURSOR_HIDDEN && cursor_mode != CURSOR_EMBEDDED {
            return Err(PortalError::InvalidArgument(format!(
                "cursor_mode {cursor_mode} is not offered"
            )));
        }
        check_session(&self.sessions, &session_handle, &owner, |state| {
            if state.starting {
                return Err(PortalError::InvalidArgument("session already started".into()));
            }
            state.cursor_mode = Some(cursor_mode);
            Ok(())
        })?;
        request::export(connection, &request, &owner).await?;
        respond_later(connection, request.clone(), owner, request::SUCCESS, HashMap::new());
        Ok(request)
    }

    #[zbus(name = "Start")]
    async fn start(
        &self,
        session_handle: OwnedObjectPath,
        _parent_window: &str,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<OwnedObjectPath, PortalError> {
        let owner = sender(&header)?;
        let request = handle::request_path(&owner, &handle::token(&options, "handle_token", &self.counter)?)?;
        let mut cursor_mode = CURSOR_HIDDEN;
        let mut epoch = 0;
        check_session(&self.sessions, &session_handle, &owner, |state| {
            let Some(mode) = state.cursor_mode else {
                return Err(PortalError::InvalidArgument("SelectSources first".into()));
            };
            if state.starting {
                return Err(PortalError::InvalidArgument("session already started".into()));
            }
            state.starting = true;
            cursor_mode = mode;
            epoch = state.epoch;
            Ok(())
        })?;
        if let Err(error) = request::export(connection, &request, &owner).await {
            set_starting(&self.sessions, session_handle.as_str(), epoch, false);
            return Err(error.into());
        }
        tokio::spawn(start_cast(
            connection.clone(),
            self.sessions.clone(),
            Target {
                session: session_handle,
                epoch,
                request: request.clone(),
                owner,
            },
            cursor_mode,
        ));
        Ok(request)
    }

    #[zbus(name = "OpenPipeWireRemote")]
    async fn open_pipewire_remote(
        &self,
        session_handle: OwnedObjectPath,
        _options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<OwnedFd, PortalError> {
        let owner = sender(&header)?;
        check_session(&self.sessions, &session_handle, &owner, |state| {
            if state.cast.is_none() {
                return Err(PortalError::InvalidArgument("session not started".into()));
            }
            Ok(())
        })?;
        let socket = remote::connect().map_err(|error| PortalError::NotFound(error.to_string()))?;
        Ok(OwnedFd::from(std::os::fd::OwnedFd::from(socket)))
    }
}

/// How long `Start` waits for compd to list its outputs. A wedged compositor
/// must not leave the request unanswered.
const OUTPUTS_TIMEOUT: Duration = Duration::from_secs(5);

/// The session and request one `Start` answers.
struct Target {
    session: OwnedObjectPath,
    /// The session's epoch when `Start` was accepted.
    epoch: u64,
    request: OwnedObjectPath,
    owner: String,
}

/// Sets `starting` on the session if it is still the same session.
fn set_starting(sessions: &Sessions, path: &str, epoch: u64, starting: bool) {
    sessions.with(|map| {
        if let Some(state) = map.get_mut(path)
            && state.epoch == epoch
        {
            state.starting = starting;
        }
    });
}

/// Answers `Start` with failure and lets the session be started again.
async fn refuse(connection: &Connection, sessions: &Sessions, target: &Target, why: &str) {
    tracing::warn!(owner = %target.owner, session = %target.session, "screencast not started: {why}");
    set_starting(sessions, target.session.as_str(), target.epoch, false);
    request::respond(connection, &target.request, &target.owner, request::FAILED, HashMap::new()).await;
}

/// Picks the source, starts the stream and answers `Start`. A cast whose
/// request was closed, or whose session went away or was replaced meanwhile,
/// is stopped.
async fn start_cast(connection: Connection, sessions: Sessions, target: Target, cursor_mode: u32) {
    let outputs = match tokio::time::timeout(OUTPUTS_TIMEOUT, tokio::task::spawn_blocking(capture::outputs)).await {
        Ok(Ok(Ok(outputs))) => outputs,
        Ok(Ok(Err(error))) => return refuse(&connection, &sessions, &target, &error.to_string()).await,
        Ok(Err(error)) => return refuse(&connection, &sessions, &target, &error.to_string()).await,
        Err(_) => return refuse(&connection, &sessions, &target, "compd did not list its outputs in time").await,
    };
    let Some(output) = outputs
        .into_iter()
        .find(|output| output.mode_width > 0 && output.mode_height > 0)
    else {
        return refuse(&connection, &sessions, &target, "compd advertises no output with a current mode").await;
    };
    let Target {
        session, owner, ..
    } = &target;
    tracing::info!(
        %owner, %session, output = %output.name,
        "granting monitor screencast (trusted session, no prompt)"
    );
    let source = output.clone();
    let cast = match tokio::task::spawn_blocking(move || {
        stream::start(source, cursor_mode == CURSOR_EMBEDDED, MAX_FRAMERATE)
    })
    .await
    {
        Ok(Ok(cast)) => cast,
        Ok(Err(error)) => return refuse(&connection, &sessions, &target, &error.to_string()).await,
        Err(error) => return refuse(&connection, &sessions, &target, &error.to_string()).await,
    };
    let node_id = cast.node_id;

    // Install the cast before answering, so OpenPipeWireRemote right after
    // the Response finds it. Only into the session Start was accepted for.
    let leftover = sessions.with(|map| match map.get_mut(session.as_str()) {
        Some(state) if state.epoch == target.epoch => {
            state.cast = Some(cast);
            None
        }
        _ => Some(cast),
    });
    if let Some(cast) = leftover {
        tokio::task::spawn_blocking(move || drop(cast));
        request::respond(&connection, &target.request, owner, request::FAILED, HashMap::new()).await;
        return;
    }

    let mut properties: HashMap<String, Value<'static>> = HashMap::new();
    properties.insert("position".into(), Value::from((output.x, output.y)));
    properties.insert("size".into(), Value::from((output.width, output.height)));
    properties.insert("source_type".into(), Value::from(SOURCE_MONITOR));
    properties.insert("id".into(), Value::from(output.name.clone()));
    let mut results = HashMap::new();
    results.insert("streams", Value::from(vec![(node_id, properties)]));
    results.insert("persist_mode", Value::from(0u32));
    if !request::respond(&connection, &target.request, owner, request::SUCCESS, results).await {
        // The owner closed the request: cancel the cast.
        let cast = sessions.with(|map| {
            map.get_mut(session.as_str())
                .filter(|state| state.epoch == target.epoch)
                .and_then(|state| {
                    state.starting = false;
                    state.cast.take()
                })
        });
        tokio::task::spawn_blocking(move || drop(cast));
        return;
    }
    tracing::info!(%owner, %session, node_id, output = %output.name, "screencast started");
    watch(connection, sessions, session.to_string(), target.epoch);
}

/// Ends the session with `Closed` once its cast stops on its own.
fn watch(connection: Connection, sessions: Sessions, session: String, epoch: u64) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let state = sessions.with(|map| {
                map.get(&session)
                    .filter(|s| s.epoch == epoch)
                    .map(|s| s.cast.as_ref().is_some_and(|c| c.ended()))
            });
            match state {
                None => return,
                Some(false) => continue,
                Some(true) => {
                    tracing::info!(%session, "screencast ended by the compositor side");
                    session::end(&connection, &sessions, &session, true).await;
                    return;
                }
            }
        }
    });
}

/// Ends every session of a caller that left the bus.
pub async fn retire(connection: &Connection, sessions: &Sessions, owner: &str) {
    for path in sessions.owned_by(owner) {
        tracing::info!(%owner, session = %path, "screencast owner left; session ended");
        session::end(connection, sessions, &path, false).await;
    }
}

/// Retires sessions whose owner leaves the bus, until the connection closes.
pub async fn watch_owners(connection: Connection, sessions: Sessions) -> zbus::Result<()> {
    use futures_util::StreamExt;
    let proxy = zbus::fdo::DBusProxy::new(&connection).await?;
    let mut changes = proxy.receive_name_owner_changed().await?;
    while let Some(change) = changes.next().await {
        let Ok(args) = change.args() else { continue };
        if args.new_owner().is_none() && args.name().as_str().starts_with(':') && !sessions.is_empty() {
            retire(&connection, &sessions, args.name().as_str()).await;
        }
    }
    Ok(())
}
