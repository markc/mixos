// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Composing and sending, on the recipe the SSR webmail proved against a
//! live server: build the RFC 5322 message, upload it as a blob, create the
//! Email in the Sent mailbox, then submit that id. The Dioxus client
//! created in Drafts with `$draft` and assumed the server moves the
//! message; maild does not, so the created email lands in Sent directly.
//!
//! The Dioxus faults fixed here: Bcc is carried through; `In-Reply-To` is
//! wrapped exactly once (a JMAP `messageId` arrives already bracketed); and
//! every header input is stripped of CR/LF before it reaches the wire
//! (header injection).

use serde_json::{Value, json};

use crate::client::{Call, Client};
use crate::error::Result;

/// What is being sent, already validated by the caller.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Composed {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    /// The message being replied to, as `Email.messageId` gives it
    /// (bracketed). Sent as `In-Reply-To`/`References`.
    pub in_reply_to: Option<String>,
}

/// Build the MIME message for [`Composed`]. Headers are CRLF-stripped;
/// `In-Reply-To` keeps exactly one pair of angle brackets; Bcc rides its
/// own header for the server's envelope to read and strip.
pub fn build_mime(composed: &Composed) -> Vec<u8> {
    let clean = |value: &str| value.replace(['\r', '\n'], " ");
    let address_list = |list: &[String]| {
        list.iter()
            .map(|a| clean(a))
            .collect::<Vec<_>>()
            .join(", ")
    };
    // A message id from the sender's domain; the angle brackets belong to
    // the caller of one_wrap, not the uuid.
    let domain = composed
        .from
        .rsplit('@')
        .next()
        .unwrap_or("localhost")
        .to_owned();
    let message_id = format!(
        "{}@{}",
        uuid::Uuid::now_v7().simple(),
        clean(&domain)
    );
    let mut mime = String::new();
    let header = |mime: &mut String, name: &str, value: &str| {
        if !value.is_empty() {
            mime.push_str(name);
            mime.push_str(": ");
            mime.push_str(value);
            mime.push_str("\r\n");
        }
    };
    header(&mut mime, "From", &clean(&composed.from));
    header(&mut mime, "To", &address_list(&composed.to));
    header(&mut mime, "Cc", &address_list(&composed.cc));
    header(&mut mime, "Bcc", &address_list(&composed.bcc));
    header(&mut mime, "Subject", &clean(&composed.subject));
    header(
        &mut mime,
        "Date",
        &chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S +0000").to_string(),
    );
    header(&mut mime, "Message-ID", &one_wrap(&message_id));
    if let Some(reply_to) = &composed.in_reply_to {
        let wrapped = one_wrap(reply_to);
        header(&mut mime, "In-Reply-To", &wrapped);
        header(&mut mime, "References", &wrapped);
    }
    mime.push_str("MIME-Version: 1.0\r\n");
    mime.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    mime.push_str("Content-Transfer-Encoding: 8bit\r\n");
    mime.push_str("\r\n");
    mime.push_str(&composed.body);
    mime.into_bytes()
}

/// Wrap a message id in angle brackets exactly once. A JMAP `messageId`
/// arrives as `"<id@host>"`; a bare `id@host` gets its brackets here.
fn one_wrap(message_id: &str) -> String {
    if message_id.starts_with('<') && message_id.ends_with('>') {
        message_id.to_owned()
    } else {
        format!("<{message_id}>")
    }
}

impl Client {
    /// The account's first submission identity, for `EmailSubmission/set`.
    /// maild seeds one; its id is the numeric string the Dioxus client
    /// hardcoded as "1" — read it instead.
    pub async fn identity_id(&self) -> Result<String> {
        let replies = self
            .call(&[Call(
                "Identity/get".into(),
                json!({ "accountId": self.account_id() }),
                "i0".into(),
            )])
            .await?;
        let data = replies.into_iter().next().ok_or_else(|| {
            crate::error::Error::Protocol("Identity/get reply is empty".into())
        })?;
        data.get("list")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(|identity| identity.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| crate::error::Error::Protocol("no identity on the account".into()))
    }

    /// Send [`Composed`]: build, upload, create in Sent, submit. Returns
    /// when the server has taken the submission.
    pub async fn send(&self, composed: &Composed, sent_mailbox_id: &str) -> Result<()> {
        let mime = build_mime(composed);
        let blob_id = self
            .upload_blob(mime, "message/rfc822")
            .await?;
        let create = json!({
            "accountId": self.account_id(),
            "create": {
                "out1": {
                    "blobId": blob_id,
                    "mailboxIds": { sent_mailbox_id: true },
                    "keywords": { "$seen": true }
                }
            }
        });
        let replies = self
            .call(&[Call("Email/set".into(), create, "c0".into())])
            .await?;
        let data = replies.into_iter().next().ok_or_else(|| {
            crate::error::Error::Protocol("Email/set reply is empty".into())
        })?;
        if let Some(error) = Client::set_error(&data, "notCreated", "out1") {
            return Err(error);
        }
        let email_id = Client::created_id(&data, "out1")?;
        // The submission references the created email; the identity comes
        // from the account, not a hardcoded id.
        let identity_id = self.identity_id().await?;
        let submit = json!({
            "accountId": self.account_id(),
            "create": {
                "sub1": {
                    "emailId": email_id,
                    "identityId": identity_id
                }
            }
        });
        let replies = self
            .call(&[Call("EmailSubmission/set".into(), submit, "s0".into())])
            .await?;
        let data = replies.into_iter().next().ok_or_else(|| {
            crate::error::Error::Protocol("EmailSubmission/set reply is empty".into())
        })?;
        if let Some(error) = Client::set_error(&data, "notCreated", "sub1") {
            return Err(error);
        }
        Client::created_id(&data, "sub1")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composed() -> Composed {
        Composed {
            from: "bob@example.invalid".into(),
            to: vec!["ann@example.invalid".into()],
            cc: vec!["carol@example.invalid".into()],
            bcc: vec!["dave@example.invalid".into()],
            subject: "Lunch".into(),
            body: "Noon?".into(),
            in_reply_to: Some("<m1@example.invalid>".into()),
        }
    }

    #[test]
    fn bcc_rides_its_own_header() {
        let mime = String::from_utf8(build_mime(&composed())).unwrap();
        assert!(mime.contains("Bcc: dave@example.invalid\r\n"));
    }

    #[test]
    fn in_reply_to_is_wrapped_exactly_once() {
        let mime = String::from_utf8(build_mime(&composed())).unwrap();
        assert!(mime.contains("In-Reply-To: <m1@example.invalid>\r\n"));
        let bare = Composed {
            in_reply_to: Some("m2@example.invalid".into()),
            ..composed()
        };
        let mime = String::from_utf8(build_mime(&bare)).unwrap();
        assert!(mime.contains("In-Reply-To: <m2@example.invalid>\r\n"));
    }

    #[test]
    fn header_injection_is_stripped() {
        let evil = Composed {
            subject: "Hi\r\nBcc: eve@example.invalid".into(),
            to: vec!["ann@example.invalid\r\nBcc: eve@example.invalid".into()],
            ..composed()
        };
        let mime = String::from_utf8(build_mime(&evil)).unwrap();
        assert!(!mime.contains("\nBcc: eve"));
        assert!(mime.contains("Subject: Hi Bcc: eve@example.invalid\r\n"));
    }

    #[test]
    fn the_body_follows_the_blank_line() {
        let mime = String::from_utf8(build_mime(&composed())).unwrap();
        let (_, body) = mime.split_once("\r\n\r\n").unwrap();
        assert_eq!(body, "Noon?");
    }
}
