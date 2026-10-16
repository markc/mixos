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
use std::time::Duration;

use reqwest::Url;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::{Account, IssueRequest, Token};
use crate::error::{Error, Result};
use crate::session::Session;

/// The token label this client asks for, so server logs name its tokens.
const LABEL: &str = "mixos-email";

/// The largest JSON body read: a token, the Session, one request's replies.
pub(crate) const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;
/// The largest blob downloaded: an attachment or a raw message.
pub(crate) const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;
/// How long a connection may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long one request may take, body included (a large upload or download
/// on a slow link fits; a server that never answers does not hang the shell).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Whether credentials may travel to this URL at all: HTTPS, or plain HTTP
/// to a loopback host only.
fn secure(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }),
        _ => false,
    }
}

/// The account's base URL, refused unless credentials may be sent to it.
pub(crate) fn account_base(url: &str) -> Result<Url> {
    let base = Url::parse(url).map_err(|e| Error::Protocol(format!("account URL {url:?}: {e}")))?;
    if !secure(&base) {
        return Err(Error::Protocol(format!(
            "account URL {}: credentials are sent only over HTTPS",
            base.origin().ascii_serialization()
        )));
    }
    Ok(base)
}

/// A server-advertised endpoint, refused unless it is on the account's own
/// origin (scheme, host and port): the Session must not redirect the
/// credentials anywhere else.
pub(crate) fn same_origin(base: &Url, url: &str) -> Result<Url> {
    let target =
        Url::parse(url).map_err(|e| Error::Protocol(format!("Session endpoint {url:?}: {e}")))?;
    if target.origin() != base.origin() {
        return Err(Error::Protocol(format!(
            "refusing to send credentials to {}: not the account's origin {}",
            target.origin().ascii_serialization(),
            base.origin().ascii_serialization()
        )));
    }
    Ok(target)
}

/// Append one chunk to a body, refusing past the cap.
fn push_capped(body: &mut Vec<u8>, chunk: &[u8], cap: usize) -> Result<()> {
    if body.len().saturating_add(chunk.len()) > cap {
        return Err(too_large(cap));
    }
    body.extend_from_slice(chunk);
    Ok(())
}

fn too_large(cap: usize) -> Error {
    Error::Protocol(format!("response body exceeds {cap} bytes"))
}

/// Read a response body, at most `cap` bytes: a declared length over the cap
/// is refused before any byte is read, and a streamed body is cut off there.
pub(crate) async fn read_capped(mut response: reqwest::Response, cap: usize) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > cap as u64) {
        return Err(too_large(cap));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        push_capped(&mut body, &chunk, cap)?;
    }
    Ok(body)
}

/// Read a bounded JSON body.
pub(crate) async fn read_json<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    Ok(serde_json::from_slice(
        &read_capped(response, MAX_JSON_BYTES).await?,
    )?)
}

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

/// The request body for `calls`, under the Session's advertised `using` list.
pub(crate) fn request_body(using: &[String], calls: &[Call]) -> Value {
    json!({ "using": using, "methodCalls": calls })
}

/// The client id every `/set` create uses. One create per call, so one id.
pub(crate) const CREATE: &str = "c0";

/// A UUID id that maild parses. maild skips a create whose parent id does not
/// parse (`calendarId`, `addressBookId`), with no reply entry, so it is
/// checked before the request goes out.
pub(crate) fn require_uuid(field: &str, value: &str) -> Result<()> {
    uuid::Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|e| Error::Protocol(format!("{field} {value:?} is not a UUID: {e}")))
}

/// A trimmed text value, or `None` when it is empty.
pub(crate) fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// A text value as JSON: `null` when empty, so a patch clears the field.
pub(crate) fn optional_text(text: &str) -> Value {
    non_empty(Some(text)).map_or(Value::Null, |text| json!(text))
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
    /// The account URL, parsed: every credentialed request stays on its origin.
    base: Url,
    /// The shared Bearer token, when one has been issued.
    token: Arc<Mutex<Option<String>>>,
    session: Session,
}

impl Client {
    /// Build the HTTP client for an account: rustls, plus the account's own
    /// CA bundle when the server's certificate is self-issued. Never a
    /// global invalid-certificate acceptance.
    fn pool_for(account: &Account) -> Result<reqwest::Client> {
        // Bounded: a server that accepts and never answers fails the request
        // rather than holding setup or sending open forever. Redirects are not
        // followed, so credentials never leave the account's origin that way.
        let mut builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none());
        if let Some(pem) = &account.ca_pem {
            // A bundle may hold several certificates (a root and its intermediate).
            let certificates = reqwest::Certificate::from_pem_bundle(pem.as_bytes())
                .map_err(|e| Error::Protocol(format!("account CA bundle: {e}")))?;
            for certificate in certificates {
                builder = builder.add_root_certificate(certificate);
            }
        }
        Ok(builder.build()?)
    }

    /// Discover the Session and exchange the password for a token. Two or
    /// three HTTPS calls; every call after this goes through [`call`].
    pub async fn connect(account: &Account) -> Result<Self> {
        let base = account_base(&account.url)?;
        let pool = Self::pool_for(account)?;
        let mut client = Self {
            pool,
            account: account.clone(),
            base,
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
    /// Basic auth. Any other failure propagates. The cell is shared, so a
    /// re-exchange from any clone is seen by them all.
    async fn exchange_token(&self) -> Result<()> {
        let url = format!(
            "{}/auth/tokens/issue",
            self.account.url.trim_end_matches('/')
        );
        let response = self
            .pool
            .post(&url)
            .basic_auth(&self.account.email, Some(&self.account.password))
            .json(&IssueRequest {
                label: LABEL.to_owned(),
            })
            .send()
            .await?;
        match response.status() {
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED => {
                // No token endpoint: a dead token held from before must go,
                // or every call 401s into this same branch forever.
                *self.token.lock().unwrap() = None;
                Ok(())
            }
            status if !status.is_success() => Err(Error::Status(status)),
            _ => {
                let token: Token = read_json(response).await?;
                // The token goes into a header on every call; refuse anything
                // that is not a plain header value.
                if token.token.is_empty() || token.token.chars().any(char::is_control) {
                    return Err(Error::Protocol(
                        "the server issued an unusable token".into(),
                    ));
                }
                *self.token.lock().unwrap() = Some(token.token);
                Ok(())
            }
        }
    }

    /// Read the Session from the well-known document. maild fronts the
    /// document with the same authentication as `/jmap`, so the request
    /// carries the token (or Basic, in the fallback mode).
    async fn read_session(&mut self) -> Result<()> {
        let url = format!(
            "{}/.well-known/jmap",
            self.account.url.trim_end_matches('/')
        );
        let response = self
            .authorize(self.pool.get(&url))
            .send()
            .await?
            .and_then_ok_status()?;
        let value: Value = read_json(response).await?;
        let session = Session::from_json(&value)?;
        // Refuse a Session that points the credentials at another origin
        // before any call is made through it.
        same_origin(&self.base, &session.api_url)?;
        same_origin(&self.base, &session.upload_url)?;
        same_origin(&self.base, &session.download_url)?;
        self.session = session;
        Ok(())
    }

    /// A Session endpoint, checked against the account's origin.
    pub(crate) fn endpoint(&self, url: &str) -> Result<Url> {
        same_origin(&self.base, url)
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
    ///
    /// The dead token stays in the cell while the exchange runs: a
    /// concurrent call keeps sending it (401, its own re-exchange) rather
    /// than dropping to Basic auth, which would cost the server a bcrypt
    /// verify per call. Two racing exchanges each issue a valid token; the
    /// last writer wins and both work.
    pub async fn call(&self, calls: &[Call]) -> Result<Vec<Value>> {
        // Whether this call carried a token, read before it is sent: a
        // concurrent fallback may clear the cell while the call is in flight.
        let sent_token = self.token.lock().unwrap().is_some();
        match self.call_once(calls).await {
            Err(Error::Unauthorized(_reason)) if sent_token => {
                // The token died mid-flight: spend another exchange and try
                // the request once more.
                self.exchange_token().await.map_err(|_| {
                    Error::Unauthorized("token expired and the re-exchange was refused".into())
                })?;
                self.call_once(calls).await
            }
            Err(Error::Unauthorized(reason)) => Err(Error::Unauthorized(reason)),
            other => other,
        }
    }

    /// One POST, no retry logic.
    async fn call_once(&self, calls: &[Call]) -> Result<Vec<Value>> {
        let api = self.endpoint(&self.session.api_url)?;
        let response = self
            .authorize(self.pool.post(api))
            .json(&request_body(&self.session.using, calls))
            .send()
            .await?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::Unauthorized("credentials refused".into()));
        }
        if !status.is_success() {
            return Err(Error::Status(status));
        }
        let reply: Reply = read_json(response).await?;
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
            failure
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("serverFail"),
            description,
        ))
    }

    /// One `/set` call: `method` with its arguments, the reply's arguments
    /// returned for the caller to check with [`check_set_reply`].
    pub(crate) async fn set_call(&self, method: &str, args: Value) -> Result<Value> {
        let replies = self.call(&[Call(method.into(), args, "s0".into())]).await?;
        replies
            .into_iter()
            .next()
            .ok_or_else(|| Error::Protocol(format!("{method} reply is empty")))
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

/// Fail with the first error a `/set` reply reports under `key` for any of
/// `ids`. A reply that names none of them is a success.
pub(crate) fn check_set_reply(data: &Value, key: &str, ids: &[String]) -> Result<()> {
    match ids.iter().find_map(|id| Client::set_error(data, key, id)) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Fail for the first of `ids` that the reply does not confirm under `key`
/// (`created`, `updated` or `destroyed`). maild's calendar and contact sets
/// skip a failed create or update without any `not*` entry, so a missing
/// confirmation is the only signal. A `destroyed` list is an array of ids;
/// the other two are maps keyed by id.
pub(crate) fn check_confirmed(data: &Value, key: &str, ids: &[String]) -> Result<()> {
    let confirmed = |id: &str| match data.get(key) {
        Some(Value::Object(map)) => map.contains_key(id),
        Some(Value::Array(list)) => list.iter().any(|entry| entry.as_str() == Some(id)),
        _ => false,
    };
    match ids.iter().find(|id| !confirmed(id)) {
        Some(id) => Err(Error::method(
            "serverFail",
            Some(&format!("server did not confirm {id:?} as {key}")),
        )),
        None => Ok(()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_travel_only_over_https_or_to_loopback() {
        assert!(account_base("https://mail.example.invalid").is_ok());
        assert!(account_base("http://127.0.0.1:8443").is_ok());
        assert!(account_base("http://[::1]:8443").is_ok());
        assert!(account_base("http://localhost:8443").is_ok());
        assert!(matches!(
            account_base("http://mail.example.invalid"),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(
            account_base("ftp://mail.example.invalid"),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn a_session_endpoint_off_the_account_origin_is_refused() {
        let base = account_base("https://mail.example.invalid").unwrap();
        assert!(same_origin(&base, "https://mail.example.invalid/jmap").is_ok());
        assert!(same_origin(&base, "https://mail.example.invalid:443/jmap/blob/{blobId}").is_ok());
        for other in [
            "https://attacker.example/jmap",
            "http://mail.example.invalid/jmap",
            "https://mail.example.invalid:8443/jmap",
            "https://sub.mail.example.invalid/jmap",
        ] {
            let refused = same_origin(&base, other).unwrap_err().to_string();
            assert!(
                refused.contains("not the account's origin"),
                "{other}: {refused}"
            );
        }
    }

    #[test]
    fn a_body_past_its_cap_is_refused() {
        let mut body = Vec::new();
        push_capped(&mut body, &[0; 6], 10).unwrap();
        push_capped(&mut body, &[0; 4], 10).unwrap();
        assert_eq!(body.len(), 10);
        assert!(matches!(
            push_capped(&mut body, &[0], 10),
            Err(Error::Protocol(_))
        ));
        assert_eq!(body.len(), 10, "nothing past the cap is kept");
    }

    #[test]
    fn the_request_body_carries_using_and_the_method_calls() {
        let calls = [Call(
            "Mailbox/get".into(),
            json!({"accountId": "A1"}),
            "m0".into(),
        )];
        let body = request_body(&["urn:ietf:params:jmap:mail".into()], &calls);
        assert_eq!(
            body,
            json!({
                "using": ["urn:ietf:params:jmap:mail"],
                "methodCalls": [["Mailbox/get", {"accountId": "A1"}, "m0"]]
            })
        );
    }

    #[test]
    fn a_set_error_for_any_listed_id_fails_the_reply() {
        let reply = json!({
            "updated": { "E1": null },
            "notUpdated": {
                "E2": { "type": "notFound", "description": "no such email" }
            }
        });
        let ids = ["E1".to_owned(), "E2".to_owned()];
        match check_set_reply(&reply, "notUpdated", &ids) {
            Err(Error::Method {
                error_type,
                description,
            }) => {
                assert_eq!(error_type, "notFound");
                assert_eq!(description, "no such email");
            }
            other => panic!("expected a method error, got {other:?}"),
        }
        assert!(check_set_reply(&reply, "notUpdated", &["E1".to_owned()]).is_ok());
    }

    #[test]
    fn a_set_error_without_a_type_is_a_server_failure() {
        let reply = json!({ "notDestroyed": { "E9": {} } });
        match check_set_reply(&reply, "notDestroyed", &["E9".to_owned()]) {
            Err(Error::Method { error_type, .. }) => assert_eq!(error_type, "serverFail"),
            other => panic!("expected a method error, got {other:?}"),
        }
    }

    #[test]
    fn a_silently_skipped_create_or_update_is_refused() {
        // maild answers a skipped create or update with no entry in either map.
        let reply = json!({ "created": { "c0": { "id": "V1" } }, "updated": null });
        assert!(check_confirmed(&reply, "created", &["c0".to_owned()]).is_ok());
        for (key, id) in [("created", "c1"), ("updated", "V1")] {
            match check_confirmed(&reply, key, &[id.to_owned()]) {
                Err(Error::Method { error_type, description }) => {
                    assert_eq!(error_type, "serverFail");
                    assert!(description.contains("did not confirm"), "{description}");
                }
                other => panic!("expected a method error for {key}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_destroyed_list_confirms_by_id() {
        let reply = json!({ "destroyed": ["M2"] });
        assert!(check_confirmed(&reply, "destroyed", &["M2".to_owned()]).is_ok());
        assert!(check_confirmed(&reply, "destroyed", &["M3".to_owned()]).is_err());
    }

    #[test]
    fn a_created_id_is_read_and_a_missing_one_is_refused() {
        let reply = json!({ "created": { "c0": { "id": "M7" } } });
        assert_eq!(Client::created_id(&reply, CREATE).unwrap(), "M7");
        assert!(matches!(
            Client::created_id(&json!({}), CREATE),
            Err(Error::Protocol(_))
        ));
    }
}
