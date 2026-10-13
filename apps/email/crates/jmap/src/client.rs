// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The connected client: one HTTPS session over the discovered endpoints.
//! [`Client::connect`] exchanges the password for a token and reads the
//! Session; [`Client::call`] POSTs any number of methodCalls as one request
//! and surfaces a method-level error triple as [`Error::Method`] before any
//! struct deserialisation.
//!
//! Quirks this absorbs (each noted where the typed surface leans on it):
//! the `using` array is the Session's own advertised capabilities, because
//! maild ignores it while a strict server validates it; a 401 with a held
//! token re-exchanges the password once before failing; and a server that
//! offers no token endpoint falls back to Basic auth on every request.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::{Account, IssueRequest, Token};
use crate::error::{Error, Result};
use crate::session::Session;

/// The token label this client asks for, so server logs name its tokens.
const LABEL: &str = "mixos-email";

/// One methodCall: method name, arguments, client call id. Serialises as
/// the protocol's `[method, args, callId]` triple.
#[derive(Clone, Debug, Serialize)]
pub struct Call(pub String, pub Value, pub String);

/// A ResultReference (RFC 8620 §3.7) into a prior call of the same request:
/// `json!({"#ids": backref("q0", "Email/query", "/ids")})` chains a query
/// into a get with no second round trip. maild resolves these against
/// top-level argument names only — a referenced value cannot land inside a
/// `/set` create entry.
pub fn backref(result_of: &str, name: &str, path: &str) -> Value {
    json!({"resultOf": result_of, "name": name, "path": path})
}

/// The request envelope.
#[derive(Serialize)]
struct Envelope<'a> {
    using: &'a [String],
    #[serde(rename = "methodCalls")]
    method_calls: &'a [Call],
}

/// The response envelope.
#[derive(Deserialize)]
struct Reply {
    #[serde(rename = "methodResponses")]
    method_responses: Vec<(String, Value, String)>,
}

/// The connected JMAP client. Cheap to clone: the HTTP pool and the token
/// cell are shared, so a re-authenticated token is seen by every clone.
#[derive(Clone)]
pub struct Client {
    pool: reqwest::Client,
    account: Account,
    /// The shared Bearer token, when one has been issued.
    token: Arc<Mutex<Option<String>>>,
    session: Session,
}

impl Client {
    /// Build the HTTP client for an account: rustls, plus the account's own
    /// CA bundle when the server's certificate is self-issued. Never a
    /// global invalid-certificate acceptance.
    fn pool_for(account: &Account) -> Result<reqwest::Client> {
        let mut builder = reqwest::Client::builder();
        if let Some(pem) = &account.ca_pem {
            let certificate = reqwest::Certificate::from_pem(pem.as_bytes())
                .map_err(|e| Error::Protocol(format!("account CA bundle: {e}")))?;
            builder = builder.add_root_certificate(certificate);
        }
        Ok(builder.build()?)
    }

    /// Discover the Session and exchange the password for a token. Two or
    /// three HTTPS calls; every call after this goes through [`call`].
    pub async fn connect(account: &Account) -> Result<Self> {
        let pool = Self::pool_for(account)?;
        let mut client = Self {
            pool,
            account: account.clone(),
            token: Arc::new(Mutex::new(None)),
            session: Session {
                using: Vec::new(),
                account_id: String::new(),
                account_name: None,
                api_url: String::new(),
                upload_url: String::new(),
                download_url: String::new(),
            },
        };
        // The exchange spends the one bcrypt-verified Basic call. A server
        // without the endpoint (a webd vhost proxying only /jmap) keeps the
        // client on Basic for every request instead.
        client.exchange_token().await?;
        client.read_session().await?;
        Ok(client)
    }

    /// Exchange the password for a token; a 404/405 leaves the client on
    /// Basic auth. Any other failure propagates.
    async fn exchange_token(&mut self) -> Result<()> {
        let url = format!("{}/auth/tokens/issue", self.account.url.trim_end_matches('/'));
        let response = self
            .http
            .post(&url)
            .basic_auth(&self.account.email, Some(&self.account.password))
            .json(&IssueRequest {
                label: LABEL.to_owned(),
            })
            .send()
            .await?;
        match response.status() {
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED => Ok(()),
            status if !status.is_success() => Err(Error::Status(status)),
            _ => {
                let token: Token = response.json().await?;
                *self.token.lock().unwrap() = Some(token.token);
                Ok(())
            }
        }
    }

    /// Read the Session from the well-known document. maild fronts the
    /// document with the same authentication as `/jmap`, so the request
    /// carries the token (or Basic, in the fallback mode).
    async fn read_session(&mut self) -> Result<()> {
        let url = format!("{}/.well-known/jmap", self.account.url.trim_end_matches('/'));
        let value = self
            .authorize(self.pool.get(&url))
            .send()
            .await?
            .and_then_ok_status()?
            .json::<Value>()
            .await?;
        self.session = Session::from_json(&value)?;
        Ok(())
    }

    /// The Session this client is connected through.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The HTTP pool, for the endpoints outside the `/jmap` envelope (blob
    /// upload and download) — always paired with [`Client::authorize`].
    pub(crate) fn pool(&self) -> &reqwest::Client {
        &self.pool
    }

    /// The account id every call addresses.
    pub fn account_id(&self) -> &str {
        &self.session.account_id
    }

    /// The Authorization value for the current mode: the held token when
    /// there is one, Basic otherwise. Used by the blob endpoints too, which
    /// sit outside `/jmap`.
    pub(crate) fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let token = self.token.lock().unwrap().clone();
        match token {
            Some(token) => request.bearer_auth(token),
            None => request.basic_auth(&self.account.email, Some(&self.account.password)),
        }
    }

    /// POST one request's methodCalls and return each response's arguments,
    /// in call order. A method-level error triple fails the whole request
    /// as [`Error::Method`]; an HTTP 401 re-exchanges the token once before
    /// giving up as [`Error::Unauthorized`].
    pub async fn call(&self, calls: &[Call]) -> Result<Vec<Value>> {
        match self.call_once(calls).await {
            Err(Error::Unauthorized(_reason)) if self.token.lock().unwrap().is_some() => {
                // The token died mid-flight: spend another exchange and try
                // the request once more.
                let mut retry = self.clone();
                *retry.token.lock().unwrap() = None;
                retry.exchange_token().await.map_err(|_| {
                    Error::Unauthorized("token expired and the re-exchange was refused".into())
                })?;
                retry.call_once(calls).await
            }
            Err(Error::Unauthorized(reason)) => Err(Error::Unauthorized(reason)),
            other => other,
        }
    }

    /// One POST, no retry logic.
    async fn call_once(&self, calls: &[Call]) -> Result<Vec<Value>> {
        let envelope = Envelope {
            using: &self.session.using,
            method_calls: calls,
        };
        let response = self
            .authorize(self.pool.post(&self.session.api_url))
            .json(&envelope)
            .send()
            .await?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::Unauthorized("credentials refused".into()));
        }
        if !status.is_success() {
            return Err(Error::Status(status));
        }
        let reply: Reply = response.json().await?;
        let mut out = Vec::with_capacity(reply.method_responses.len());
        for (name, value, _) in reply.method_responses {
            if name == "error" {
                let error_type = value.get("type").and_then(Value::as_str).unwrap_or("error");
                let description = value.get("description").and_then(Value::as_str);
                return Err(Error::method(error_type, description));
            }
            out.push(value);
        }
        Ok(out)
    }

    /// The error map a `/set` reply carries for one client id (`notCreated`,
    /// `notUpdated`, `notDestroyed`), when the server reported one.
    pub(crate) fn set_error(data: &Value, key: &str, client_id: &str) -> Option<Error> {
        let failure = data.get(key)?.as_object()?.get(client_id)?;
        let description = failure.get("description").and_then(Value::as_str);
        Some(Error::method(
            failure.get("type").and_then(Value::as_str).unwrap_or("serverFail"),
            description,
        ))
    }

    /// The server id a `/set` reply assigned to one created client id. A
    /// silent skip (maild's PIM handlers never populate `notCreated`) reads
    /// as a protocol error: the id simply is not there.
    pub(crate) fn created_id(data: &Value, client_id: &str) -> Result<String> {
        data.get("created")
            .and_then(|created| created.get(client_id))
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::Protocol(format!("no id for created {client_id:?}")))
    }
}

/// `and_then` sugar for the status check that precedes every body read.
trait StatusCheck {
    fn and_then_ok_status(self) -> Result<reqwest::Response>;
}

impl StatusCheck for reqwest::Response {
    fn and_then_ok_status(self) -> Result<reqwest::Response> {
        let status = self.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::Unauthorized("credentials refused".into()));
        }
        if !status.is_success() {
            return Err(Error::Status(status));
        }
        Ok(self)
    }
}
