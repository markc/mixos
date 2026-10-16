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
use crate::mail::EmailAddress;

/// What is being sent, already validated by the caller. Recipients are
/// parsed ([`parse_addresses`]): the header shows each name and address,
/// the submission envelope takes only the address.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Composed {
    pub from: String,
    pub to: Vec<EmailAddress>,
    pub cc: Vec<EmailAddress>,
    pub bcc: Vec<EmailAddress>,
    pub subject: String,
    pub body: String,
    /// The message being replied to, as `Email.messageId` gives it
    /// (bracketed). Sent as `In-Reply-To`/`References`.
    pub in_reply_to: Option<String>,
}

/// Parse a recipient field: entries separated by `,` or `;` outside quotes
/// and angle brackets, each `addr`, `<addr>`, `Name <addr>` or
/// `"Last, First" <addr>`. Empty entries are skipped. A malformed entry is
/// refused with a message naming it.
pub fn parse_addresses(text: &str) -> std::result::Result<Vec<EmailAddress>, String> {
    let mut entries = Vec::new();
    let mut current = String::new();
    let (mut quoted, mut escaped, mut angle) = (false, false, false);
    for c in text.chars() {
        if escaped {
            escaped = false;
        } else if quoted && c == '\\' {
            escaped = true;
        } else if c == '"' && !angle {
            quoted = !quoted;
        } else if !quoted && c == '<' {
            if angle {
                return Err(format!("nested '<' in {text:?}"));
            }
            angle = true;
        } else if !quoted && c == '>' {
            angle = false;
        } else if !quoted && !angle && (c == ',' || c == ';') {
            entries.push(std::mem::take(&mut current));
            continue;
        }
        current.push(c);
    }
    if quoted || angle {
        return Err(format!("unterminated quote or '<' in {:?}", current.trim()));
    }
    entries.push(current);
    entries
        .iter()
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(parse_entry)
        .collect()
}

fn parse_entry(entry: &str) -> std::result::Result<EmailAddress, String> {
    let refuse = || format!("not an email address: {entry:?}");
    let (name, email) = match entry.rfind('<') {
        Some(open) => {
            let email = entry[open + 1..].strip_suffix('>').ok_or_else(refuse)?;
            let name = entry[..open].trim();
            let name = match name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) {
                Some(inner) => unescape(inner),
                None if name.contains('"') => return Err(refuse()),
                None => name.to_owned(),
            };
            ((!name.is_empty()).then_some(name), email.trim())
        }
        None => (None, entry),
    };
    if !is_addr_spec(email) {
        return Err(refuse());
    }
    Ok(EmailAddress {
        name,
        email: email.to_owned(),
    })
}

/// Undo a quoted string's backslash escapes.
fn unescape(inner: &str) -> String {
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    out
}

/// A plain `local@domain`: one `@`, both sides present, and nothing that
/// would break a header or an envelope (whitespace, controls, specials).
fn is_addr_spec(email: &str) -> bool {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !local.contains('@')
        && email
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !"<>()[],;:\"\\".contains(c))
}

/// One address as a header (and the compose field) writes it:
/// `Name <addr>`, the name quoted when it holds a special character, or
/// the bare address. [`parse_addresses`] reads it back.
pub fn format_address(address: &EmailAddress) -> String {
    match address.name.as_deref().filter(|name| !name.is_empty()) {
        Some(name) if name.chars().any(|c| "()<>[]:;@\\,.\"".contains(c)) => {
            let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\" <{}>", address.email)
        }
        Some(name) => format!("{name} <{}>", address.email),
        None => address.email.clone(),
    }
}

/// Build the MIME message for [`Composed`]. Headers are CRLF-stripped;
/// `In-Reply-To` keeps exactly one pair of angle brackets; Bcc rides its
/// own header for the server's envelope to read and strip.
pub fn build_mime(composed: &Composed) -> Vec<u8> {
    let clean = |value: &str| value.replace(['\r', '\n'], " ");
    let address_list = |list: &[EmailAddress]| {
        list.iter()
            .map(|a| clean(&format_address(a)))
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
    let message_id = format!("{}@{}", uuid::Uuid::now_v7().simple(), clean(&domain));
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
    // No Bcc header: the bytes go to every recipient. Bcc recipients travel
    // in the submission envelope instead (see `send`).
    header(&mut mime, "Subject", &clean(&composed.subject));
    header(
        &mut mime,
        "Date",
        &chrono::Utc::now()
            .format("%a, %d %b %Y %H:%M:%S +0000")
            .to_string(),
    );
    header(&mut mime, "Message-ID", &one_wrap(&message_id));
    if let Some(reply_to) = &composed.in_reply_to {
        // The id came from the server, not the person: sanitise it like the rest.
        let wrapped = one_wrap(&clean(reply_to));
        header(&mut mime, "In-Reply-To", &wrapped);
        header(&mut mime, "References", &wrapped);
    }
    mime.push_str("MIME-Version: 1.0\r\n");
    mime.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    mime.push_str("Content-Transfer-Encoding: 8bit\r\n");
    mime.push_str("\r\n");
    // RFC 5322 wants CRLF on every line; a text editor hands over bare LF.
    mime.push_str(&composed.body.replace("\r\n", "\n").replace('\n', "\r\n"));
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
    /// The submission identity to send `from` with, for `EmailSubmission/set`:
    /// the one whose address is `from`, else the account's first. The Dioxus
    /// client hardcoded the id as "1"; it is read from the account instead.
    pub async fn identity_id(&self, from: &str) -> Result<String> {
        let replies = self
            .call(&[Call(
                "Identity/get".into(),
                json!({ "accountId": self.account_id() }),
                "i0".into(),
            )])
            .await?;
        let data = replies
            .into_iter()
            .next()
            .ok_or_else(|| crate::error::Error::Protocol("Identity/get reply is empty".into()))?;
        let list = data
            .get("list")
            .and_then(Value::as_array)
            .ok_or_else(|| crate::error::Error::Protocol("Identity/get has no list".into()))?;
        // The identity must carry the sender's address. No fallback: sending
        // under another identity would put a From that is not ours on the wire.
        list.iter()
            .find(|identity| {
                identity
                    .get("email")
                    .and_then(Value::as_str)
                    .is_some_and(|email| email.eq_ignore_ascii_case(from))
            })
            .and_then(|identity| identity.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                crate::error::Error::Protocol(format!("no submission identity for {from}"))
            })
    }

    /// Send [`Composed`]: identify, build, upload, create in Sent, submit.
    /// The identity is fetched first: after the create, a failure would
    /// strand the message in Sent with nothing submitted. Returns when the
    /// server has taken the submission.
    pub async fn send(&self, composed: &Composed, sent_mailbox_id: &str) -> Result<()> {
        let identity_id = self.identity_id(&composed.from).await?;
        let mime = build_mime(composed);
        let blob_id = self.upload_blob(mime, "message/rfc822").await?;
        // A top-level `blobId` on `Email/set` create is maild's extension:
        // RFC 8621 builds an Email from uploaded MIME with `Email/import`
        // instead, so a strict server refuses this. Sending requires maild;
        // `Email/import` is future work.
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
        let data = replies
            .into_iter()
            .next()
            .ok_or_else(|| crate::error::Error::Protocol("Email/set reply is empty".into()))?;
        if let Some(error) = Client::set_error(&data, "notCreated", "out1") {
            return Err(error);
        }
        let email_id = Client::created_id(&data, "out1")?;
        // The submission references the created email; the identity was
        // read from the account before anything was created.
        // The envelope names every recipient, Bcc included, so Bcc people
        // receive the message without appearing in its bytes.
        let rcpt_to: Vec<Value> = composed
            .to
            .iter()
            .chain(&composed.cc)
            .chain(&composed.bcc)
            .map(|address| json!({ "email": address.email }))
            .collect();
        let submit = json!({
            "accountId": self.account_id(),
            "create": {
                "sub1": {
                    "emailId": email_id,
                    "identityId": identity_id,
                    "envelope": {
                        "mailFrom": { "email": composed.from },
                        "rcptTo": rcpt_to
                    }
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

    fn bare(email: &str) -> EmailAddress {
        EmailAddress {
            name: None,
            email: email.into(),
        }
    }

    fn named(name: &str, email: &str) -> EmailAddress {
        EmailAddress {
            name: Some(name.into()),
            email: email.into(),
        }
    }

    fn composed() -> Composed {
        Composed {
            from: "bob@example.invalid".into(),
            to: vec![bare("ann@example.invalid")],
            cc: vec![bare("carol@example.invalid")],
            bcc: vec![bare("dave@example.invalid")],
            subject: "Lunch".into(),
            body: "Noon?".into(),
            in_reply_to: Some("<m1@example.invalid>".into()),
        }
    }

    #[test]
    fn bcc_stays_out_of_the_bytes() {
        let mime = String::from_utf8(build_mime(&composed())).unwrap();
        assert!(
            !mime.contains("Bcc:"),
            "the bytes must not name Bcc recipients"
        );
        assert!(!mime.contains("dave@example.invalid"));
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
            to: vec![bare("ann@example.invalid\r\nBcc: eve@example.invalid")],
            ..composed()
        };
        let mime = String::from_utf8(build_mime(&evil)).unwrap();
        assert!(!mime.contains("\nBcc: eve"));
        // Each stripped control character leaves one space.
        assert!(mime.contains("Subject: Hi  Bcc: eve@example.invalid\r\n"));
    }

    #[test]
    fn recipients_parse_bare_named_and_quoted_forms() {
        assert_eq!(
            parse_addresses(
                "ann@example.invalid; Bob <bob@example.invalid>, \"Lee, Cy\" <cy@example.invalid>,"
            ),
            Ok(vec![
                bare("ann@example.invalid"),
                named("Bob", "bob@example.invalid"),
                named("Lee, Cy", "cy@example.invalid"),
            ])
        );
        assert_eq!(
            parse_addresses("<dee@example.invalid>; \"Q \\\"x\\\" R\" <q@example.invalid>"),
            Ok(vec![
                bare("dee@example.invalid"),
                named("Q \"x\" R", "q@example.invalid"),
            ])
        );
        assert_eq!(parse_addresses("  ,; "), Ok(Vec::new()));
    }

    #[test]
    fn malformed_recipients_are_refused() {
        for bad in [
            "ann",
            "ann@",
            "@example.invalid",
            "Ann <ann@example.invalid",
            "\"Ann <ann@example.invalid>",
            "Ann <ann example@invalid>",
            "Ann <a@b@c>",
            "An\"n <ann@example.invalid>",
            "<<ann@example.invalid>>",
        ] {
            assert!(parse_addresses(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn headers_show_names_and_quote_specials() {
        let message = Composed {
            to: vec![
                named("Lee, Cy", "cy@example.invalid"),
                named("Bob", "bob@example.invalid"),
            ],
            cc: Vec::new(),
            ..composed()
        };
        let mime = String::from_utf8(build_mime(&message)).unwrap();
        assert!(mime.contains("To: \"Lee, Cy\" <cy@example.invalid>, Bob <bob@example.invalid>\r\n"));
    }

    #[test]
    fn the_body_follows_the_blank_line() {
        let mime = String::from_utf8(build_mime(&composed())).unwrap();
        let (_, body) = mime.split_once("\r\n\r\n").unwrap();
        assert_eq!(body, "Noon?");
    }
}
