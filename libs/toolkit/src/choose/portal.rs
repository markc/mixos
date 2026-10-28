// SPDX-License-Identifier: MIT OR Apache-2.0
//! Direct FileChooser v4 client. A new connection and owner probe per call;
//! subscription precedes the method, and a visible Request has no deadline.
use super::*;
use anyhow::{Context as _, Result, anyhow};
use futures_util::StreamExt;
use std::collections::HashMap;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

const DESKTOP: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.FileChooser";
const REQUEST: &str = "org.freedesktop.portal.Request";
type Options = HashMap<String, OwnedValue>;
type WireFilter = (String, Vec<(u32, String)>);

fn filter(value: &Filter) -> WireFilter {
    (
        value.name.clone(),
        value
            .patterns()
            .iter()
            .map(|p| (0, p.clone()))
            .chain(value.mime_types.iter().map(|p| (1, p.clone())))
            .collect(),
    )
}

fn put(options: &mut Options, key: &str, value: Value<'_>) -> Result<()> {
    options.insert(key.into(), OwnedValue::try_from(value)?);
    Ok(())
}

fn nul(bytes: &[u8]) -> Vec<u8> {
    let mut result = bytes.to_vec();
    result.push(0);
    result
}

fn options(request: &Request, token: &str) -> Result<Options> {
    let mut options = Options::new();
    put(&mut options, "handle_token", Value::from(token))?;
    put(
        &mut options,
        "accept_label",
        Value::from(request.accept_label.as_str()),
    )?;
    if request.mode == Mode::Open {
        put(&mut options, "multiple", Value::from(request.multiple))?;
        put(&mut options, "directory", Value::from(request.directory))?;
    }
    if request.mode != Mode::SaveFiles {
        if request.filter_menu {
            put(
                &mut options,
                "filters",
                Value::from(request.filters.iter().map(filter).collect::<Vec<_>>()),
            )?;
        }
        if let Some(current) = &request.current_filter {
            put(&mut options, "current_filter", Value::from(filter(current)))?;
        }
    } else {
        put(
            &mut options,
            "files",
            Value::from(request.files.iter().map(|n| nul(n)).collect::<Vec<_>>()),
        )?;
    }
    let choices: Vec<_> = request
        .choices
        .iter()
        .map(|c| {
            (
                c.id.clone(),
                c.label.clone(),
                c.options
                    .iter()
                    .map(|o| (o.id.clone(), o.label.clone()))
                    .collect::<Vec<_>>(),
                c.default.clone(),
            )
        })
        .collect();
    if !choices.is_empty() {
        put(&mut options, "choices", Value::from(choices))?;
    }
    if let Some(folder) = &request.current_folder {
        put(&mut options, "current_folder", Value::from(nul(folder)))?;
    }
    if let Some(file) = &request.current_file {
        put(&mut options, "current_file", Value::from(nul(file)))?;
    }
    if let Some(name) = &request.current_name {
        if let Ok(name) = std::str::from_utf8(name) {
            put(&mut options, "current_name", Value::from(name))?;
        } else {
            // current_name is a D-Bus string; current_file is the byte-preserving
            // equivalent when a name is not UTF-8. Resolve its parent on this worker.
            let folder = request
                .current_folder
                .as_deref()
                .map(path_from_bytes)
                .unwrap_or_else(crate::file_chooser::home);
            put(
                &mut options,
                "current_file",
                Value::from(nul(&path_bytes(&folder.join(path_from_bytes(name))))),
            )?;
        }
    }
    Ok(options)
}

pub(super) async fn attempt(
    request: &Request,
    cancel: &mut watch::Receiver<bool>,
) -> Option<Outcome> {
    let connection = tokio::time::timeout(IO_BOUND, Connection::session())
        .await
        .ok()?
        .ok()?;
    match session(&connection, request, cancel).await {
        Ok(outcome) => outcome,
        Err(_) => None,
    }
}

pub(super) async fn session(
    connection: &Connection,
    request: &Request,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Option<Outcome>> {
    session_inner(
        connection,
        request,
        cancel,
        #[cfg(test)]
        0,
    )
    .await
}

async fn session_inner(
    connection: &Connection,
    request: &Request,
    cancel: &mut watch::Receiver<bool>,
    #[cfg(test)] mut break_stream: u8,
) -> Result<Option<Outcome>> {
    let dbus = tokio::time::timeout(IO_BOUND, zbus::fdo::DBusProxy::new(connection)).await??;
    if !tokio::time::timeout(IO_BOUND, dbus.name_has_owner(DESKTOP.try_into()?)).await?? {
        return Ok(None);
    }
    let owner = tokio::time::timeout(IO_BOUND, dbus.get_name_owner(DESKTOP.try_into()?)).await??;
    let proxy = tokio::time::timeout(
        IO_BOUND,
        zbus::Proxy::new(connection, owner.as_str(), PATH, INTERFACE),
    )
    .await??;
    let version: u32 = tokio::time::timeout(IO_BOUND, proxy.get_property("version")).await??;
    let required = if request.mode == Mode::SaveFiles {
        4
    } else if request.directory {
        3
    } else {
        1
    };
    if version < required {
        return Ok(None);
    }
    let token = format!("toolkit{}", uuid::Uuid::now_v7().simple());
    let sender = connection
        .unique_name()
        .context("session connection has no unique name")?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let predicted = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    // Include all paths from this captured owner: old implementations may return
    // a different handle. Queuing before the call also retains an early Response.
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(owner.as_str())?
        .interface(REQUEST)?
        .member("Response")?
        .build();
    let mut responses = tokio::time::timeout(
        IO_BOUND,
        MessageStream::for_match_rule(rule.clone(), connection, Some(8)),
    )
    .await??;
    if *cancel.borrow() {
        return Ok(None);
    }
    let method = match request.mode {
        Mode::Open => "OpenFile",
        Mode::Save => "SaveFile",
        Mode::SaveFiles => "SaveFiles",
    };
    let args = (
        &request.parent_window,
        &request.title,
        options(request, &token)?,
    );
    let call = connection.call_method(Some(owner.as_str()), PATH, Some(INTERFACE), method, &args);
    tokio::pin!(call);
    let reply = tokio::select! {
        reply = &mut call => reply,
        _ = cancelled(cancel) => {
            // Close the predicted path promptly, then retain the method future
            // long enough to close a different/late returned handle as well.
            let _ = close(connection, owner.as_str(), &predicted).await;
            if let Ok(Ok(reply)) = tokio::time::timeout(IO_BOUND, &mut call).await
                && let Ok(actual) = reply.body().deserialize::<OwnedObjectPath>() {
                let _ = close(connection, owner.as_str(), actual.as_str()).await;
            }
            return Ok(None);
        }
    };
    let (actual, mut recovering): (OwnedObjectPath, bool) = match reply {
        Ok(reply) => match reply.body().deserialize() {
            Ok(actual) => (actual, false),
            Err(_) => (predicted.try_into()?, true),
        },
        Err(zbus::Error::MethodError(_, _, _)) => return Ok(None),
        // A lost acknowledgement may still have opened the predicted Request.
        Err(_) => (predicted.try_into()?, true),
    };
    let mut responses_live = true;
    let mut rebuilt = false;
    let mut sweep = tokio::time::interval(Duration::from_secs(5));
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancelled(cancel) => {
                let _ = close(connection, owner.as_str(), actual.as_str()).await;
                return Ok(None);
            }
            next = async {
                #[cfg(test)]
                if break_stream > 0 { break_stream -= 1; return None; }
                responses.next().await
            }, if responses_live => {
                let message = match next {
                    Some(Ok(message)) => message,
                    _ => {
                        // A broken signal stream alone is not permission to
                        // overlap the still open Request with another chooser.
                        if !rebuilt {
                            rebuilt = true;
                            if let Ok(Ok(stream)) = tokio::time::timeout(IO_BOUND,
                                MessageStream::for_match_rule(rule.clone(), connection, Some(8))).await {
                                responses = stream;
                                continue;
                            }
                        }
                        responses_live = false;
                        recovering = true;
                        continue;
                    }
                };
                if message.header().path().is_none_or(|p| p.as_str() != actual.as_str()) { continue; }
                let (code, values): (u32, Options) = match message.body().deserialize() {
                    Ok(body) => body,
                    Err(error) => return Ok(Some(Outcome::Failed(error.to_string()))),
                };
                // Every Response, including 2, ends the Request before fallback.
                return Ok(if code == 2 { None } else {
                    Some(decode(request, code, &values).unwrap_or_else(|e| Outcome::Failed(e.to_string())))
                });
            }
            _ = sweep.tick() => {
                if recovering && close(connection, owner.as_str(), actual.as_str()).await.is_ok() {
                    return Err(anyhow!("portal request cancelled after transport failure"));
                }
                match tokio::time::timeout(IO_BOUND, dbus.get_name_owner(DESKTOP.try_into()?)).await {
                    Ok(Ok(current)) if current == owner => {},
                    Ok(_) => return Err(anyhow!("portal owner lost")),
                    Err(_) => { /* A stalled bus does not end a person's window. */ }
                }
            }
        }
    }
}

pub(super) async fn close(connection: &Connection, owner: &str, path: &str) -> Result<()> {
    let reply = tokio::time::timeout(
        IO_BOUND,
        connection.call_method(Some(owner), path, Some(REQUEST), "Close", &()),
    )
    .await?;
    match reply {
        Ok(_) => Ok(()),
        Err(zbus::Error::MethodError(name, _, _))
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.UnknownObject"
                    | "org.freedesktop.DBus.Error.UnknownMethod"
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) fn decode(request: &Request, code: u32, values: &Options) -> Result<Outcome> {
    if code == 1 {
        return Ok(Outcome::Cancelled);
    }
    if code != 0 {
        return Ok(Outcome::Failed(format!("portal Response {code}")));
    }
    let mut value = serde_json::json!({"response":0});
    value["uris"] = serde_json::json!(Vec::<String>::try_from(
        values
            .get("uris")
            .context("portal omitted uris")?
            .try_clone()?
    )?);
    if let Some(choices) = values.get("choices") {
        value["choices"] = serde_json::json!(
            Vec::<(String, String)>::try_from(choices.try_clone()?)?
                .into_iter()
                .collect::<BTreeMap<_, _>>()
        );
    }
    if let Some(current) = values.get("current_filter") {
        let (name, _) = WireFilter::try_from(current.try_clone()?)?;
        // GTK and KDE may normalise the tuple's entries. The displayed name
        // identifies the offered filter; retain its original contract shape.
        let selected = request
            .filters
            .iter()
            .chain(request.current_filter.iter())
            .find(|filter| filter.name == name)
            .context("portal returned an unknown filter name")?;
        value["current_filter"] = serde_json::to_value(selected)?;
    }
    Ok(decode_outcome(request, &value))
}

#[cfg(test)]
pub(super) async fn session_broken_stream(
    connection: &Connection,
    request: &Request,
    cancel: &mut watch::Receiver<bool>,
    breaks: u8,
) -> Result<Option<Outcome>> {
    session_inner(connection, request, cancel, breaks).await
}
