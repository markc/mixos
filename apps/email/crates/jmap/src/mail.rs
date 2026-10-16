// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Mail: the RFC 8621 types and the calls the reader drives. The wire
//! structs and the pure helpers are ported from the Dioxus client; the
//! pagination, body fetch, mark-read and download behaviour is new.
//!
//! maild notes the types lean on: `from`/`to`/`cc` addresses arrive as
//! `EmailAddress {name, email}` through webmail's own writer, but the
//! submission envelope treats them as bare addr-specs; `threadId` and
//! `preview` are always null, so neither is read here; Email/query takes a
//! flat filter (no FilterOperator) and one sort comparator.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Call, Client};
use crate::error::{Error, Result};

/// One mailbox, as `Mailbox/get` returns it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    #[serde(rename = "parentId", default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(rename = "sortOrder", default)]
    pub sort_order: i32,
    #[serde(rename = "totalEmails", default)]
    pub total_emails: u64,
    #[serde(rename = "unreadEmails", default)]
    pub unread_emails: u64,
}

/// One address, as Email headers carry them.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EmailAddress {
    #[serde(default)]
    pub name: Option<String>,
    pub email: String,
}

/// One body part pointer; the bytes live in `bodyValues`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BodyPart {
    #[serde(rename = "partId")]
    pub part_id: String,
    #[serde(rename = "type", default)]
    pub content_type: Option<String>,
}

/// One email, as `Email/get` returns it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Email {
    pub id: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub from: Option<Vec<EmailAddress>>,
    #[serde(default)]
    pub to: Option<Vec<EmailAddress>>,
    #[serde(default)]
    pub cc: Option<Vec<EmailAddress>>,
    #[serde(rename = "messageId", default)]
    pub message_id: Option<Vec<String>>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(rename = "receivedAt", default)]
    pub received_at: Option<String>,
    #[serde(rename = "hasAttachment", default)]
    pub has_attachment: Option<bool>,
    #[serde(default)]
    pub keywords: Option<Value>,
    #[serde(rename = "bodyValues", default)]
    pub body_values: Option<serde_json::Map<String, Value>>,
    #[serde(rename = "textBody", default)]
    pub text_body: Option<Vec<BodyPart>>,
    #[serde(rename = "htmlBody", default)]
    pub html_body: Option<Vec<BodyPart>>,
}

impl Email {
    /// The first text body's value, fetched with `fetchTextBodyValues`.
    pub fn text_body_value(&self) -> Option<&str> {
        self.body_value(Self::text_part)
    }

    /// The first HTML body's value, fetched with `fetchHTMLBodyValues`.
    pub fn html_body_value(&self) -> Option<&str> {
        self.body_value(Self::html_part)
    }

    fn text_part(&self) -> Option<&BodyPart> {
        self.text_body.as_ref()?.first()
    }

    fn html_part(&self) -> Option<&BodyPart> {
        self.html_body.as_ref()?.first()
    }

    fn body_value(&self, part: impl Fn(&Self) -> Option<&BodyPart>) -> Option<&str> {
        let values = self.body_values.as_ref()?;
        let part = part(self)?;
        values.get(&part.part_id)?.get("value")?.as_str()
    }

    /// The first From address's display form: its name when set, else the
    /// address.
    pub fn from_display(&self) -> String {
        self.from
            .as_ref()
            .and_then(|addrs| addrs.first())
            .map(|a| match a.name.as_deref() {
                Some(name) if !name.is_empty() => name.to_owned(),
                _ => a.email.clone(),
            })
            .unwrap_or_default()
    }

    /// `date`, else `receivedAt`, as a short local stamp.
    pub fn date_short(&self) -> String {
        self.date
            .as_deref()
            .or(self.received_at.as_deref())
            .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
            .map(|dt| dt.format("%b %d %H:%M").to_string())
            .unwrap_or_default()
    }

    /// Whether the `$seen` keyword is set.
    pub fn is_read(&self) -> bool {
        self.keywords
            .as_ref()
            .and_then(|k| k.get("$seen"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

/// A mailbox listing page: the ids' emails and the full match count behind
/// them. `position` counts from the newest under the default sort.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmailPage {
    pub emails: Vec<Email>,
    /// The whole mailbox's match count, not just this page's.
    pub total: u64,
}

impl Client {
    /// Every mailbox on the account, role-sorted by the server.
    pub async fn mailboxes(&self) -> Result<Vec<Mailbox>> {
        let replies = self
            .call(&[Call(
                "Mailbox/get".into(),
                json!({ "accountId": self.account_id() }),
                "m0".into(),
            )])
            .await?;
        let data = replies.into_iter().next().ok_or(empty("Mailbox/get"))?;
        Ok(parse_get::<Mailbox>(data)?.0)
    }

    /// One page of a mailbox's email, newest first, with the full count.
    /// One round trip: `Email/query` chains into `Email/get` through a
    /// ResultReference.
    pub async fn email_page(
        &self,
        mailbox_id: &str,
        position: u32,
        limit: u32,
    ) -> Result<EmailPage> {
        let limit = limit.clamp(1, 500);
        let replies = self
            .call(&[
                Call(
                    "Email/query".into(),
                    json!({
                        "accountId": self.account_id(),
                        "filter": { "inMailbox": mailbox_id },
                        "sort": [{ "property": "receivedAt", "isAscending": false }],
                        "position": position,
                        "limit": limit,
                        "calculateTotal": true,
                    }),
                    "q0".into(),
                ),
                Call(
                    "Email/get".into(),
                    json!({
                        "accountId": self.account_id(),
                        "#ids": crate::client::backref("q0", "Email/query", "/ids"),
                        "fetchTextBodyValues": false,
                        "fetchHTMLBodyValues": false,
                    }),
                    "e0".into(),
                ),
            ])
            .await?;
        let mut replies = replies.into_iter();
        let query = replies.next().ok_or(empty("Email/query"))?;
        let get = replies.next().ok_or(empty("Email/get"))?;
        let total = query.get("total").and_then(Value::as_u64).unwrap_or(0);
        let emails = parse_get::<Email>(get)?.0;
        Ok(EmailPage { emails, total })
    }

    /// One email with its bodies fetched.
    pub async fn email(&self, id: &str) -> Result<Email> {
        let replies = self
            .call(&[Call(
                "Email/get".into(),
                json!({
                    "accountId": self.account_id(),
                    "ids": [id],
                    "fetchTextBodyValues": true,
                    "fetchHTMLBodyValues": true,
                }),
                "e0".into(),
            )])
            .await?;
        let data = replies.into_iter().next().ok_or(empty("Email/get"))?;
        let (list, not_found) = parse_get::<Email>(data)?;
        if list.is_empty() && not_found {
            return Err(Error::Protocol(format!("email {id:?} is gone")));
        }
        list.into_iter()
            .next()
            .ok_or_else(|| Error::Protocol("email get returned no entry".into()))
    }

    /// Add or remove the `$seen` keyword (mark read / unread) as a keyword
    /// pointer delta, the form maild's keyword bitmap patcher accepts.
    pub async fn mark_read(&self, id: &str, read: bool) -> Result<()> {
        let patch = if read {
            json!({ "keywords/$seen": true })
        } else {
            json!({ "keywords/$seen": null })
        };
        self.update_email(id, patch).await
    }

    /// Update email properties (keywords, mailboxIds) with one patch.
    pub async fn update_email(&self, id: &str, patch: Value) -> Result<()> {
        let replies = self
            .call(&[Call(
                "Email/set".into(),
                json!({
                    "accountId": self.account_id(),
                    "update": { id: patch },
                }),
                "u0".into(),
            )])
            .await?;
        let data = replies.into_iter().next().ok_or(empty("Email/set"))?;
        if let Some(error) = Client::set_error(&data, "notUpdated", id) {
            return Err(error);
        }
        Ok(())
    }

    /// Permanently destroy an email.
    pub async fn destroy_email(&self, id: &str) -> Result<()> {
        let replies = self
            .call(&[Call(
                "Email/set".into(),
                json!({
                    "accountId": self.account_id(),
                    "destroy": [id],
                }),
                "d0".into(),
            )])
            .await?;
        let data = replies.into_iter().next().ok_or(empty("Email/set"))?;
        if let Some(error) = Client::set_error(&data, "notDestroyed", id) {
            return Err(error);
        }
        Ok(())
    }

    /// Upload raw bytes as a blob and return its id.
    pub async fn upload_blob(&self, data: Vec<u8>, content_type: &str) -> Result<String> {
        let upload = self.endpoint(&self.session().upload_for(self.account_id()))?;
        let response = self
            .authorize(
                self.pool()
                    .post(upload)
                    .header(reqwest::header::CONTENT_TYPE, content_type)
                    .body(data),
            )
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Status(status));
        }
        #[derive(Deserialize)]
        struct Uploaded {
            #[serde(rename = "blobId")]
            blob_id: String,
        }
        let uploaded: Uploaded = crate::client::read_json(response).await?;
        Ok(uploaded.blob_id)
    }

    /// Download one blob's bytes (an attachment, a raw message), at most
    /// [`crate::client::MAX_BLOB_BYTES`].
    pub async fn download_blob(&self, blob_id: &str) -> Result<Vec<u8>> {
        let download = self.endpoint(&self.session().download_for(blob_id))?;
        let response = self
            .authorize(self.pool().get(download))
            .send()
            .await?
            .error_for_status()?;
        crate::client::read_capped(response, crate::client::MAX_BLOB_BYTES).await
    }
}

/// A `/get` reply: the list, and whether the server reported ids it could
/// not find (maild answers `notFound: []` on the PIM types, so absence of
/// entries there is the only signal).
fn parse_get<T: serde::de::DeserializeOwned>(data: Value) -> Result<(Vec<T>, bool)> {
    let not_found = data
        .get("notFound")
        .and_then(Value::as_array)
        .is_some_and(|ids| !ids.is_empty());
    let list = data
        .get("list")
        .cloned()
        .ok_or_else(|| Error::Protocol("get reply has no list".into()))?;
    Ok((serde_json::from_value(list)?, not_found))
}

fn empty(method: &str) -> Error {
    Error::Protocol(format!("{method} reply is empty"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_dioxus_email_shape() {
        let email: Email = serde_json::from_value(json!({
            "id": "E1",
            "subject": "Lunch",
            "from": [{ "name": "Ann", "email": "ann@example.invalid" }],
            "to": [{ "name": null, "email": "bob@example.invalid" }],
            "messageId": ["<m1@example.invalid>"],
            "receivedAt": "2026-10-13T01:02:03Z",
            "hasAttachment": true,
            "keywords": { "$seen": true },
            "bodyValues": { "p1": { "value": "Sandwiches?", "partId": "p1" } },
            "textBody": [{ "partId": "p1", "type": "text/plain" }],
            "htmlBody": []
        }))
        .unwrap();
        assert_eq!(email.from_display(), "Ann");
        assert!(email.is_read());
        assert_eq!(email.text_body_value(), Some("Sandwiches?"));
        assert_eq!(email.html_body_value(), None);
        assert!(!email.date_short().is_empty());
    }

    #[test]
    fn a_missing_from_falls_back_to_the_address() {
        let email: Email = serde_json::from_value(json!({
            "id": "E2",
            "from": [{ "name": "", "email": "bob@example.invalid" }]
        }))
        .unwrap();
        assert_eq!(email.from_display(), "bob@example.invalid");
        assert!(!email.is_read());
    }
}
