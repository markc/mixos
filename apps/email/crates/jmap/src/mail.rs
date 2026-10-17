// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Mail: the RFC 8621 types and the calls the reader drives. The wire
//! structs and the pure helpers are ported from the Dioxus client; the
//! pagination, body fetch, mark-read and download behaviour is new.
//!
//! maild notes the types lean on: its Email/get projects `from` as a bare
//! string and `to`/`cc` as lists of bare addr-specs (the RFC 8621
//! `EmailAddress` object form is pending there), so the address lists read
//! either shape; the submission envelope treats them as bare addr-specs;
//! `threadId` and
//! `preview` are always null, so neither is read here; Email/query takes a
//! flat filter (no FilterOperator) and one sort comparator.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{CREATE, Call, Client, check_confirmed, check_set_reply};
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

/// An address list as the server sends it: RFC 8621's `[EmailAddress]`, or
/// maild's projection while its object form is pending, which is a bare
/// string for `from` and a list of bare strings elsewhere. A string entry is
/// read as `addr` or `Name <addr>`; one that does not parse is kept whole as
/// the address, so a listing never fails on a header it cannot split.
fn addresses<'de, D>(deserializer: D) -> std::result::Result<Option<Vec<EmailAddress>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    fn entry(value: Value, out: &mut Vec<EmailAddress>) {
        match value {
            Value::String(text) => match crate::parse_addresses(&text) {
                Ok(parsed) if !parsed.is_empty() => out.extend(parsed),
                _ if text.trim().is_empty() => {}
                _ => out.push(EmailAddress {
                    name: None,
                    email: text,
                }),
            },
            Value::Object(_) => {
                if let Ok(address) = serde_json::from_value(value) {
                    out.push(address);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(items)) => items.into_iter().for_each(|v| entry(v, &mut out)),
        Some(other) => entry(other, &mut out),
    }
    Ok(Some(out))
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
    #[serde(default, deserialize_with = "addresses")]
    pub from: Option<Vec<EmailAddress>>,
    #[serde(default, deserialize_with = "addresses")]
    pub to: Option<Vec<EmailAddress>>,
    #[serde(default, deserialize_with = "addresses")]
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
    /// The parts that are not the body (RFC 8621 `attachments`).
    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,
}

/// One attachment: a part that is not the body. maild marks a part it
/// could not decode `undecodable` and gives it no blob.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Attachment {
    #[serde(rename = "partId", default)]
    pub part_id: Option<String>,
    #[serde(rename = "blobId", default)]
    pub blob_id: Option<String>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type", default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub disposition: Option<String>,
    #[serde(default)]
    pub cid: Option<String>,
    #[serde(default)]
    pub undecodable: bool,
}

impl Attachment {
    /// Whether its bytes can be fetched.
    pub fn downloadable(&self) -> bool {
        !self.undecodable && self.blob_id.as_deref().is_some_and(|b| !b.is_empty())
    }
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
    /// A thin wrapper over [`Client::email_query`] with no text filter.
    pub async fn email_page(
        &self,
        mailbox_id: &str,
        position: u32,
        limit: u32,
    ) -> Result<EmailPage> {
        self.email_query(
            Some(mailbox_id),
            None,
            u64::from(position),
            u64::from(limit),
        )
        .await
    }

    /// One page of the email matching an optional mailbox and an optional
    /// full-text query, newest first, with the full match count. `None` for
    /// both lists the whole account. One round trip: `Email/query` chains
    /// into `Email/get` through a ResultReference.
    pub async fn email_query(
        &self,
        mailbox: Option<&str>,
        text: Option<&str>,
        position: u64,
        limit: u64,
    ) -> Result<EmailPage> {
        let calls = email_query_calls(
            self.account_id(),
            email_filter(mailbox, text),
            position,
            limit,
        );
        let replies = self.call(&calls).await?;
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

    /// Add or remove the `$seen` keyword (mark read / unread). A wrapper over
    /// [`Client::set_keywords`].
    pub async fn mark_read(&self, id: &str, read: bool) -> Result<()> {
        self.set_keywords(&[id.to_owned()], Some(read), None).await
    }

    /// Set or clear `$seen` (`read`) and `$flagged` (`flagged`) on every id
    /// in one Email/set. `None` leaves that keyword alone. Each keyword is a
    /// pointer delta, the form maild's keyword bitmap patcher accepts. A
    /// refused id fails the call with the server's error. A no-op update is
    /// omitted from `updated` by RFC 8620 §5.3, so success is not confirmed
    /// here: only the `notUpdated` map is checked.
    pub async fn set_keywords(
        &self,
        ids: &[String],
        read: Option<bool>,
        flagged: Option<bool>,
    ) -> Result<()> {
        if ids.is_empty() || (read.is_none() && flagged.is_none()) {
            return Ok(());
        }
        let args = email_update_args(self.account_id(), ids, &keyword_patch(read, flagged));
        let data = self.set_call("Email/set", args).await?;
        check_set_reply(&data, "notUpdated", ids)
    }

    /// Move every id into exactly one mailbox: `mailboxIds` becomes
    /// `{to: true}`, dropping any other membership.
    pub async fn move_emails(&self, ids: &[String], to_mailbox: &str) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let patch = json!({ "mailboxIds": { to_mailbox: true } });
        let args = email_update_args(self.account_id(), ids, &patch);
        let data = self.set_call("Email/set", args).await?;
        check_set_reply(&data, "notUpdated", ids)
    }

    /// Delete every id. With `destroy`, the emails are destroyed; without it
    /// they move to the mailbox with the `trash` role, which must exist.
    pub async fn delete_emails(&self, ids: &[String], destroy: bool) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        if !destroy {
            let mailboxes = self.mailboxes().await?;
            let trash = trash_mailbox(&mailboxes)
                .ok_or_else(|| Error::Protocol("no mailbox has the trash role".into()))?;
            return self.move_emails(ids, &trash.id).await;
        }
        let args = json!({ "accountId": self.account_id(), "destroy": ids });
        let data = self.set_call("Email/set", args).await?;
        check_set_reply(&data, "notDestroyed", ids)?;
        check_confirmed(&data, "destroyed", ids)
    }

    /// Create a mailbox under `parent` (or at the top level) and return its id.
    pub async fn create_mailbox(&self, name: &str, parent: Option<&str>) -> Result<String> {
        let args = mailbox_create_args(self.account_id(), name, parent);
        let data = self.set_call("Mailbox/set", args).await?;
        check_set_reply(&data, "notCreated", &[CREATE.to_owned()])?;
        check_confirmed(&data, "created", &[CREATE.to_owned()])?;
        Client::created_id(&data, CREATE)
    }

    /// Rename a mailbox.
    pub async fn rename_mailbox(&self, id: &str, name: &str) -> Result<()> {
        let args = mailbox_rename_args(self.account_id(), id, name);
        let data = self.set_call("Mailbox/set", args).await?;
        check_set_reply(&data, "notUpdated", &[id.to_owned()])?;
        check_confirmed(&data, "updated", &[id.to_owned()])
    }

    /// Delete a mailbox. Refused while it still holds email
    /// (`onDestroyRemoveEmails` is false): the emails are never removed with it.
    pub async fn delete_mailbox(&self, id: &str) -> Result<()> {
        let args = mailbox_destroy_args(self.account_id(), id);
        let data = self.set_call("Mailbox/set", args).await?;
        check_set_reply(&data, "notDestroyed", &[id.to_owned()])?;
        check_confirmed(&data, "destroyed", &[id.to_owned()])
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

/// The Email/query filter: the mailbox and the text, each only when given.
/// An empty object matches every email.
fn email_filter(mailbox: Option<&str>, text: Option<&str>) -> Value {
    let mut filter = json!({});
    if let Some(mailbox) = mailbox {
        filter["inMailbox"] = json!(mailbox);
    }
    if let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) {
        filter["text"] = json!(text);
    }
    filter
}

/// The Email/query and Email/get pair behind [`Client::email_query`]: one
/// round trip, the get chained to the query's ids.
fn email_query_calls(account_id: &str, filter: Value, position: u64, limit: u64) -> Vec<Call> {
    vec![
        Call(
            "Email/query".into(),
            json!({
                "accountId": account_id,
                "filter": filter,
                "sort": [{ "property": "receivedAt", "isAscending": false }],
                "position": position,
                "limit": limit.clamp(1, 500),
                "calculateTotal": true,
            }),
            "q0".into(),
        ),
        Call(
            "Email/get".into(),
            json!({
                "accountId": account_id,
                "#ids": crate::client::backref("q0", "Email/query", "/ids"),
                "fetchTextBodyValues": false,
                "fetchHTMLBodyValues": false,
            }),
            "e0".into(),
        ),
    ]
}

/// The keyword patch for `$seen` and `$flagged`: `true` sets a keyword,
/// `null` clears it, and `None` leaves it out of the patch entirely.
fn keyword_patch(read: Option<bool>, flagged: Option<bool>) -> Value {
    let mut patch = json!({});
    for (keyword, set) in [("$seen", read), ("$flagged", flagged)] {
        if let Some(set) = set {
            patch[format!("keywords/{keyword}")] = if set { json!(true) } else { Value::Null };
        }
    }
    patch
}

/// The Email/set arguments applying one patch to every id.
fn email_update_args(account_id: &str, ids: &[String], patch: &Value) -> Value {
    let update: serde_json::Map<String, Value> =
        ids.iter().map(|id| (id.clone(), patch.clone())).collect();
    json!({ "accountId": account_id, "update": update })
}

/// The mailbox with the `trash` role, when the account has one.
fn trash_mailbox(mailboxes: &[Mailbox]) -> Option<&Mailbox> {
    mailboxes
        .iter()
        .find(|mailbox| mailbox.role.as_deref() == Some("trash"))
}

/// The Mailbox/set arguments creating one mailbox, client id [`CREATE`].
fn mailbox_create_args(account_id: &str, name: &str, parent: Option<&str>) -> Value {
    json!({
        "accountId": account_id,
        "create": { CREATE: { "name": name, "parentId": parent } },
    })
}

/// The Mailbox/set arguments renaming one mailbox.
fn mailbox_rename_args(account_id: &str, id: &str, name: &str) -> Value {
    json!({ "accountId": account_id, "update": { id: { "name": name } } })
}

/// The Mailbox/set arguments destroying one mailbox, refused while it still
/// holds email.
fn mailbox_destroy_args(account_id: &str, id: &str) -> Value {
    json!({
        "accountId": account_id,
        "destroy": [id],
        "onDestroyRemoveEmails": false,
    })
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
    fn reads_maild_bare_string_addresses() {
        let email: Email = serde_json::from_value(json!({
            "id": "E2",
            "from": "annie@example.invalid",
            "to": ["bob@example.invalid", "Cy Lee <cy@example.invalid>", ""],
            "cc": [],
            "keywords": {}
        }))
        .unwrap();
        assert_eq!(
            email.from,
            Some(vec![EmailAddress {
                name: None,
                email: "annie@example.invalid".into()
            }])
        );
        let to = email.to.unwrap();
        assert_eq!(to.len(), 2);
        assert_eq!(to[1].name.as_deref(), Some("Cy Lee"));
        assert_eq!(to[1].email, "cy@example.invalid");
        assert_eq!(email.cc, Some(Vec::new()));
        // Absent and null stay None.
        let bare: Email = serde_json::from_value(json!({ "id": "E3", "from": null })).unwrap();
        assert_eq!(bare.from, None);
        assert_eq!(bare.to, None);
    }

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
    fn reads_maild_attachments() {
        let email: Email = serde_json::from_value(json!({
            "id": "E1",
            "attachments": [
                { "partId": "2", "blobId": "B2", "size": 1234, "name": "plan.pdf",
                  "type": "application/pdf", "disposition": "attachment", "cid": null },
                { "partId": "3", "size": 10, "name": "odd.bin",
                  "type": "application/octet-stream", "undecodable": true }
            ]
        }))
        .unwrap();
        let attachments = email.attachments.unwrap();
        assert_eq!(attachments[0].blob_id.as_deref(), Some("B2"));
        assert_eq!(attachments[0].size, 1234);
        assert_eq!(
            attachments[0].content_type.as_deref(),
            Some("application/pdf")
        );
        assert!(attachments[0].downloadable());
        assert!(attachments[1].undecodable && !attachments[1].downloadable());
        // An email without the property reads as none.
        let bare: Email = serde_json::from_value(json!({ "id": "E2" })).unwrap();
        assert!(bare.attachments.is_none());
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

    #[test]
    fn the_query_pair_is_one_request_with_the_filter_applied() {
        let calls = email_query_calls("A1", email_filter(Some("M1"), Some("  invoice ")), 20, 500);
        assert_eq!(
            crate::client::request_body(&["urn:ietf:params:jmap:mail".into()], &calls),
            json!({
                "using": ["urn:ietf:params:jmap:mail"],
                "methodCalls": [
                    ["Email/query", {
                        "accountId": "A1",
                        "filter": { "inMailbox": "M1", "text": "invoice" },
                        "sort": [{ "property": "receivedAt", "isAscending": false }],
                        "position": 20,
                        "limit": 500,
                        "calculateTotal": true
                    }, "q0"],
                    ["Email/get", {
                        "accountId": "A1",
                        "#ids": { "resultOf": "q0", "name": "Email/query", "path": "/ids" },
                        "fetchTextBodyValues": false,
                        "fetchHTMLBodyValues": false
                    }, "e0"]
                ]
            })
        );
    }

    #[test]
    fn no_mailbox_and_no_text_matches_the_whole_account_at_a_clamped_limit() {
        let calls = email_query_calls("A1", email_filter(None, Some("   ")), 0, 0);
        let body = serde_json::to_value(&calls).unwrap();
        assert_eq!(body[0][1]["filter"], json!({}));
        assert_eq!(body[0][1]["limit"], json!(1));
        let calls = email_query_calls("A1", email_filter(None, None), 0, 9_000);
        let body = serde_json::to_value(&calls).unwrap();
        assert_eq!(body[0][1]["limit"], json!(500));
    }

    #[test]
    fn a_keyword_patch_sets_clears_or_leaves_each_keyword() {
        assert_eq!(
            keyword_patch(Some(true), Some(false)),
            json!({ "keywords/$seen": true, "keywords/$flagged": null })
        );
        assert_eq!(
            keyword_patch(None, Some(true)),
            json!({ "keywords/$flagged": true })
        );
        assert_eq!(
            keyword_patch(Some(false), None),
            json!({ "keywords/$seen": null })
        );
    }

    #[test]
    fn a_keyword_update_applies_the_same_patch_to_each_id() {
        let ids = ["E1".to_owned(), "E2".to_owned()];
        assert_eq!(
            email_update_args("A1", &ids, &keyword_patch(Some(true), None)),
            json!({
                "accountId": "A1",
                "update": {
                    "E1": { "keywords/$seen": true },
                    "E2": { "keywords/$seen": true }
                }
            })
        );
    }

    #[test]
    fn a_move_replaces_the_whole_mailbox_set() {
        let ids = ["E1".to_owned()];
        let patch = json!({ "mailboxIds": { "M9": true } });
        assert_eq!(
            email_update_args("A1", &ids, &patch),
            json!({ "accountId": "A1", "update": { "E1": { "mailboxIds": { "M9": true } } } })
        );
    }

    #[test]
    fn the_trash_mailbox_is_the_one_with_the_trash_role() {
        let mailboxes = vec![
            Mailbox {
                id: "M1".into(),
                role: Some("inbox".into()),
                ..Mailbox::default()
            },
            Mailbox {
                id: "M2".into(),
                role: Some("trash".into()),
                ..Mailbox::default()
            },
        ];
        assert_eq!(trash_mailbox(&mailboxes).map(|m| m.id.as_str()), Some("M2"));
        assert!(trash_mailbox(&mailboxes[..1]).is_none());
    }

    #[test]
    fn mailbox_set_arguments_name_the_mailbox_and_keep_its_emails() {
        assert_eq!(
            mailbox_create_args("A1", "Projects", Some("M1")),
            json!({
                "accountId": "A1",
                "create": { "c0": { "name": "Projects", "parentId": "M1" } }
            })
        );
        assert_eq!(
            mailbox_create_args("A1", "Top", None)["create"]["c0"]["parentId"],
            Value::Null
        );
        assert_eq!(
            mailbox_rename_args("A1", "M2", "Archive"),
            json!({ "accountId": "A1", "update": { "M2": { "name": "Archive" } } })
        );
        assert_eq!(
            mailbox_destroy_args("A1", "M2"),
            json!({
                "accountId": "A1",
                "destroy": ["M2"],
                "onDestroyRemoveEmails": false
            })
        );
    }
}
