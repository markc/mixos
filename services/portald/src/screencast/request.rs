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

pub struct Request {
    owner: String,
}

impl Request {
    pub fn new(owner: &str) -> Self {
        Self {
            owner: owner.to_owned(),
        }
    }
}

#[zbus::interface(name = "org.freedesktop.portal.Request")]
impl Request {
    #[zbus(name = "Close")]
    async fn close(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(object_server)] server: &ObjectServer,
    ) -> fdo::Result<()> {
        let sender = header.sender().map(|name| name.as_str()).unwrap_or("");
        if sender != self.owner {
            return Err(fdo::Error::AccessDenied("not the owner of this request".into()));
        }
        let Some(path) = header.path() else {
            return Err(fdo::Error::Failed("no object path".into()));
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
pub async fn export(connection: &Connection, path: &OwnedObjectPath, owner: &str) -> zbus::Result<()> {
    if !connection
        .object_server()
        .at(path.clone(), Request::new(owner))
        .await?
    {
        return Err(zbus::Error::Failure(format!("request {path} already exists")));
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
    let emitted = async {
        let destination = BusName::try_from(owner.to_owned()).map_err(zbus::Error::from)?;
        let emitter = SignalEmitter::new(connection, path.clone())?.set_destination(destination);
        Request::response(&emitter, code, results).await
    }
    .await;
    if let Err(error) = emitted {
        tracing::warn!(%error, %path, "Response not emitted");
    }
    let _ = connection
        .object_server()
        .remove::<Request, _>(path.clone())
        .await;
    true
}
