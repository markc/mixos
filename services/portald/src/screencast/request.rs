// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `org.freedesktop.portal.Request`: one exported object per asynchronous
//! call. It is exported before its path is returned, answered once with a
//! destination-addressed `Response`, then removed. `Close` from the owner
//! removes it without a `Response`.
use std::collections::HashMap;
use std::time::Duration;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::{ObjectServer, SignalEmitter};
use zbus::zvariant::{OwnedObjectPath, Value};
use zbus::{Connection, fdo};

/// The user granted the request.
pub const SUCCESS: u32 = 0;
/// The user cancelled.
pub const CANCELLED: u32 = 1;
/// The request ended some other way: no source, capture failure.
pub const FAILED: u32 = 2;

#[derive(Debug)]
pub enum CloseError { Fdo(fdo::Error), NotAllowed }

impl From<zbus::Error> for CloseError {
    fn from(error: zbus::Error) -> Self { Self::Fdo(error.into()) }
}

impl zbus::DBusError for CloseError {
    fn name(&self) -> zbus::names::ErrorName<'_> {
        match self {
            Self::Fdo(error) => zbus::DBusError::name(error),
            Self::NotAllowed => zbus::names::ErrorName::try_from("org.freedesktop.portal.Error.NotAllowed").expect("constant"),
        }
    }
    fn description(&self) -> Option<&str> {
        match self { Self::Fdo(error) => zbus::DBusError::description(error), Self::NotAllowed => Some("not the owner of this request") }
    }
    fn create_reply(&self, header: &Header<'_>) -> zbus::Result<zbus::Message> {
        match self {
            Self::Fdo(error) => zbus::DBusError::create_reply(error, header),
            Self::NotAllowed => zbus::Message::error(header, self.name())?.build(&self.description().unwrap_or_default()),
        }
    }
}

pub struct Request {
    owner: String,
    chooser: Option<(crate::filechooser::Bridge, u64)>,
}

impl Request {
    pub fn new(owner: &str) -> Self {
        Self {
            owner: owner.to_owned(),
            chooser: None,
        }
    }

    pub fn filechooser(owner: &str, bridge: crate::filechooser::Bridge, id: u64) -> Self {
        Self { owner: owner.to_owned(), chooser: Some((bridge, id)) }
    }
}

#[zbus::interface(name = "org.freedesktop.portal.Request")]
impl Request {
    #[zbus(name = "Close")]
    async fn close(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CloseError> {
        let sender = header.sender().map(|name| name.as_str()).unwrap_or("");
        if self.chooser.is_some() && sender != self.owner {
            return Err(CloseError::NotAllowed);
        }
        if sender != self.owner {
            return Err(CloseError::Fdo(fdo::Error::AccessDenied(
                "not the owner of this request".into(),
            )));
        }
        if let Some((bridge, id)) = &self.chooser {
            bridge.close(connection, *id).await;
            return Ok(());
        }
        let Some(path) = header.path() else {
            return Err(CloseError::Fdo(fdo::Error::Failed("no object path".into())));
        };
        server.remove::<Request, _>(path.to_owned()).await?;
        Ok(())
    }

    #[zbus(signal, name = "Response")]
    async fn response(
        emitter: &SignalEmitter<'_>,
        response: u32,
        results: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<()>;
}

/// Exports a request object for `owner` at `path`.
pub async fn export(
    connection: &Connection,
    path: &OwnedObjectPath,
    owner: &str,
) -> zbus::Result<()> {
    if !connection
        .object_server()
        .at(path.clone(), Request::new(owner))
        .await?
    {
        return Err(zbus::Error::Failure(format!(
            "request {path} already exists"
        )));
    }
    Ok(())
}

/// Whether the request is still exported, i.e. its owner has not closed it.
pub async fn live(connection: &Connection, path: &OwnedObjectPath) -> bool {
    connection
        .object_server()
        .interface::<_, Request>(path.clone())
        .await
        .is_ok()
}

/// Answers the request and removes it. Returns false when the owner had
/// already closed it, so the caller can undo whatever it prepared.
pub async fn respond(
    connection: &Connection,
    path: &OwnedObjectPath,
    owner: &str,
    code: u32,
    results: HashMap<&str, Value<'_>>,
) -> bool {
    // The method reply carrying `path` must reach the caller before the
    // signal; zbus sends it once the handler returns, so give it that turn.
    tokio::time::sleep(Duration::from_millis(5)).await;
    if !live(connection, path).await {
        return false;
    }
    emit(connection, path, owner, code, results).await;
    let _ = connection
        .object_server()
        .remove::<Request, _>(path.clone())
        .await;
    true
}

/// Shared addressed signal emission. The FileChooser registry serialises this
/// with Close and retirement; ScreenCast retains its existing liveness check.
pub async fn emit(connection: &Connection, path: &OwnedObjectPath, owner: &str, code: u32, results: HashMap<&str, Value<'_>>) {
    let emitted = async {
        let destination = BusName::try_from(owner.to_owned()).map_err(zbus::Error::from)?;
        let emitter = SignalEmitter::new(connection, path.clone())?.set_destination(destination);
        Request::response(&emitter, code, results).await
    }
    .await;
    if let Err(error) = emitted {
        tracing::warn!(%error, %path, "Response not emitted");
    }
}
