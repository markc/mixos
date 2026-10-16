// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Session object (`GET /.well-known/jmap`, RFC 8620 §2): the server's
//! advertised capabilities, the one account a credential reaches, and the
//! URL templates every later call is routed through. Nothing in this crate
//! hardcodes an endpoint path; the Dioxus client's `/jmap` and
//! `/jmap/upload/1` literals are gone.

use serde_json::Value;

use crate::{Error, Result};

/// The capability URNs this client works with. maild advertises the
/// draft-era calendars/contacts URNs and ignores the request's `using`
/// array entirely; a strict server validates it, so the request sends the
/// URNs the server itself advertised in the Session (see [`Session::using`]).
pub mod urn {
    /// RFC 8620 core.
    pub const CORE: &str = "urn:ietf:params:jmap:core";
    /// RFC 8621 mail.
    pub const MAIL: &str = "urn:ietf:params:jmap:mail";
    /// RFC 8621 submission.
    pub const SUBMISSION: &str = "urn:ietf:params:jmap:submission";
    /// Calendars. maild: the draft URN; the data model is JSCalendar
    /// (RFC 8984) shaped.
    pub const CALENDARS: &str = "urn:ietf:params:jmap:calendars";
    /// Contacts. maild: the draft URN; the data model is JSContact
    /// (RFC 9553) shaped.
    pub const CONTACTS: &str = "urn:ietf:params:jmap:contacts";
}

/// One server's Session: capabilities, the account, and the URL templates.
#[derive(Clone, Debug)]
pub struct Session {
    /// The capability URNs the server advertised, in its order. Sent back
    /// verbatim as each request's `using`.
    pub using: Vec<String>,
    /// The account id every call addresses. maild scopes by the
    /// authenticated credential and never cross-checks the wire argument;
    /// a strict server requires the right one, so it is always sent.
    pub account_id: String,
    /// The account's display name, when the Session carried one.
    pub account_name: Option<String>,
    /// Where JMAP requests are POSTed (absolute).
    pub api_url: String,
    /// The blob upload template, carrying `{accountId}`.
    pub upload_url: String,
    /// The blob download template, carrying `{blobId}` (and, on some
    /// servers, a name).
    pub download_url: String,
}

impl Session {
    /// Read the Session out of the well-known document.
    pub fn from_json(value: &Value) -> Result<Self> {
        let template = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|url| !url.is_empty())
                .ok_or_else(|| Error::Protocol(format!("Session has no {key}")))
        };
        let using = value
            .get("capabilities")
            .and_then(Value::as_object)
            .map(|caps| caps.keys().cloned().collect())
            .ok_or_else(|| Error::Protocol("Session has no capabilities".into()))?;
        // The account the mail capability points at; every URN maild maps
        // lands on the same one, but read it through `primaryAccounts` like
        // the spec says rather than guessing the accounts map's key.
        let account_id = value
            .get("primaryAccounts")
            .and_then(|accounts| accounts.get(urn::MAIL))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::Protocol("Session has no mail primaryAccount".into()))?;
        let account_name = value
            .get("accounts")
            .and_then(Value::as_object)
            .and_then(|accounts| accounts.get(&account_id))
            .and_then(|account| account.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        Ok(Self {
            using,
            account_id,
            account_name,
            api_url: template("apiUrl")?,
            upload_url: template("uploadUrl")?,
            download_url: template("downloadUrl")?,
        })
    }

    /// The upload URL with this Session's account filled in.
    pub fn upload_for(&self, account_id: &str) -> String {
        self.upload_url.replace("{accountId}", account_id)
    }

    /// The download URL for one blob.
    pub fn download_for(&self, blob_id: &str) -> String {
        self.download_url.replace("{blobId}", blob_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn session() -> Value {
        json!({
            "capabilities": {
                "urn:ietf:params:jmap:core": { "maxSizeUpload": 50000000 },
                "urn:ietf:params:jmap:mail": {},
                "urn:ietf:params:jmap:submission": {},
                "urn:ietf:params:jmap:calendars": {},
                "urn:ietf:params:jmap:contacts": {}
            },
            "accounts": { "7": { "name": "Account 7", "isPersonal": true, "isReadOnly": false } },
            "primaryAccounts": {
                "urn:ietf:params:jmap:mail": "7",
                "urn:ietf:params:jmap:submission": "7",
                "urn:ietf:params:jmap:calendars": "7",
                "urn:ietf:params:jmap:contacts": "7"
            },
            "username": "",
            "apiUrl": "https://example.invalid/jmap",
            "downloadUrl": "https://example.invalid/jmap/blob/{blobId}",
            "uploadUrl": "https://example.invalid/jmap/upload/{accountId}",
            "eventSourceUrl": "https://example.invalid/jmap/eventsource",
            "state": "0"
        })
    }

    #[test]
    fn reads_the_maild_session_shape() {
        let s = Session::from_json(&session()).unwrap();
        assert_eq!(s.account_id, "7");
        assert_eq!(s.account_name.as_deref(), Some("Account 7"));
        assert_eq!(s.using.len(), 5);
        assert_eq!(s.api_url, "https://example.invalid/jmap");
        assert_eq!(s.upload_for("7"), "https://example.invalid/jmap/upload/7");
        assert_eq!(s.download_for("B1"), "https://example.invalid/jmap/blob/B1");
    }

    #[test]
    fn a_session_without_templates_is_a_protocol_error() {
        let mut value = session();
        value["apiUrl"] = json!("");
        assert!(matches!(
            Session::from_json(&value),
            Err(Error::Protocol(_))
        ));
    }
}
