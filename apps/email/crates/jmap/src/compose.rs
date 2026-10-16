// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Composing and sending, on the recipe the SSR webmail proved against a
//! live server: build the RFC 5322 message, upload it as a blob, create the
//! Email in the Sent mailbox, then submit that id. The Dioxus client
//! created in Drafts with `$draft` and assumed the server moves the
//! message; maild does not, so the created email lands in Sent directly.
//!
//! A draft is saved on the same recipe minus the submission: created in
//! the Drafts mailbox as `$draft $seen`, with its Bcc kept in the bytes, the
//! draft it replaces destroyed in the same Email/set.
//!
//! The Dioxus faults fixed here: Bcc is carried through; `In-Reply-To` is
//! wrapped exactly once (a JMAP `messageId` arrives already bracketed); and
//! every header input is stripped of CR/LF before it reaches the wire
//! (header injection).

use serde_json::{Value, json};

use crate::client::{Call, Client, check_confirmed, check_set_reply};
use crate::error::Result;
use crate::mail::{Email, EmailAddress};

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
    let split = split_addresses(text);
    if split.nested {
        return Err(format!("nested '<' in {text:?}"));
    }
    if split.open {
        let last = split.entries.last().map_or("", |range| &text[range.clone()]);
        return Err(format!("unterminated quote or '<' in {:?}", last.trim()));
    }
    split
        .entries
        .iter()
        .map(|range| text[range.clone()].trim())
        .filter(|entry| !entry.is_empty())
        .map(parse_entry)
        .collect()
}

/// A recipient field cut into its entries, by [`split_addresses`].
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    /// Each entry's byte range, untrimmed, empty ones included: the text
    /// between `,`/`;` separators outside quotes and angle brackets. There
    /// is always at least one, the last running to the end of the text.
    pub entries: Vec<std::ops::Range<usize>>,
    /// A quote or `<` is still open at the end.
    pub open: bool,
    /// A `<` opened inside another.
    pub nested: bool,
}

/// Cut a recipient field into entries, the rules [`parse_addresses`] reads
/// it by. Unlike the parse it never fails: a field being typed is cut too
/// (its last entry is the one autocomplete works on).
pub fn split_addresses(text: &str) -> Split {
    let mut entries = Vec::new();
    let mut start = 0;
    let (mut quoted, mut escaped, mut angle, mut nested) = (false, false, false, false);
    for (at, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && c == '\\' {
            escaped = true;
        } else if c == '"' && !angle {
            quoted = !quoted;
        } else if !quoted && c == '<' {
            nested |= angle;
            angle = true;
        } else if !quoted && c == '>' {
            angle = false;
        } else if !quoted && !angle && (c == ',' || c == ';') {
            entries.push(start..at);
            start = at + c.len_utf8();
        }
    }
    entries.push(start..text.len());
    Split {
        entries,
        open: quoted || angle,
        nested,
    }
}

/// A recipient field as a draft keeps it: every entry that parses as an
/// address, and every one that does not kept whole as typed (a draft is
/// saved half-written; only a send needs the field to parse).
pub fn draft_addresses(text: &str) -> Vec<EmailAddress> {
    split_addresses(text)
        .entries
        .iter()
        .map(|range| text[range.clone()].trim())
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            parse_entry(entry).unwrap_or_else(|_| EmailAddress {
                name: None,
                email: entry.to_owned(),
            })
        })
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
/// `In-Reply-To` keeps exactly one pair of angle brackets; Bcc is left out
/// of the bytes (the submission envelope carries it).
pub fn build_mime(composed: &Composed) -> Vec<u8> {
    mime(composed, false)
}

/// Build a draft's MIME message: [`build_mime`]'s, plus a `Bcc` header, so
/// the Bcc recipients come back when the draft is resumed. A draft is
/// never submitted from these bytes; a send builds them again without it.
pub fn build_draft_mime(composed: &Composed) -> Vec<u8> {
    mime(composed, true)
}

fn mime(composed: &Composed, bcc: bool) -> Vec<u8> {
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
    // No Bcc header on a message to send: the bytes go to every recipient.
    // Bcc recipients travel in the submission envelope instead (see `send`).
    if bcc {
        header(&mut mime, "Bcc", &address_list(&composed.bcc));
    }
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

    /// Save [`Composed`] as a draft in `drafts_mailbox`, replacing the
    /// draft `replaces` names: the bytes are uploaded, then ONE Email/set
    /// creates the new draft (`$draft $seen`) and destroys the old one.
    /// The create must be confirmed, or the save failed. A destroy that is
    /// not confirmed does not fail it: the new draft stands, and
    /// [`SavedDraft::kept`] says why the old one is still there.
    pub async fn save_draft(
        &self,
        composed: &Composed,
        drafts_mailbox: &str,
        replaces: Option<&str>,
    ) -> Result<SavedDraft> {
        let blob_id = self
            .upload_blob(build_draft_mime(composed), "message/rfc822")
            .await?;
        let args = draft_set_args(self.account_id(), &blob_id, drafts_mailbox, replaces);
        let data = self.set_call("Email/set", args).await?;
        draft_outcome(&data, replaces)
    }

    /// A saved draft, read back for the compose dialog: its recipients,
    /// subject, text body and the message it replies to. The raw message
    /// is read too, for the `In-Reply-To` and `Bcc` a server does not
    /// project (maild's `inReplyTo` is `null` for now).
    pub async fn resume_draft(&self, id: &str) -> Result<Composed> {
        let email = self.email(id).await?;
        let raw = match &email.blob_id {
            Some(blob) => Some(self.download_blob(blob).await?),
            None => None,
        };
        Ok(resumed(&email, raw.as_deref()))
    }
}

/// The client id of a draft save's create.
const DRAFT: &str = "draft";

/// What a draft save made.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedDraft {
    /// The new draft's id.
    pub id: String,
    /// Why the draft it replaced is still on the server, when it is.
    pub kept: Option<String>,
}

/// The Email/set arguments of a draft save: the uploaded bytes created in
/// Drafts as a read draft, and the draft they replace destroyed.
fn draft_set_args(
    account_id: &str,
    blob_id: &str,
    drafts_mailbox: &str,
    replaces: Option<&str>,
) -> Value {
    let mut args = json!({
        "accountId": account_id,
        "create": {
            DRAFT: {
                "blobId": blob_id,
                "mailboxIds": { drafts_mailbox: true },
                "keywords": { "$draft": true, "$seen": true }
            }
        }
    });
    if let Some(old) = replaces {
        args["destroy"] = json!([old]);
    }
    args
}

/// A draft save's reply: the create confirmed (maild drops a failed create
/// without a `notCreated` entry, so the missing confirmation is checked
/// too), and the destroy confirmed or the reason it was not.
fn draft_outcome(data: &Value, replaces: Option<&str>) -> Result<SavedDraft> {
    let draft = [DRAFT.to_owned()];
    check_set_reply(data, "notCreated", &draft)?;
    check_confirmed(data, "created", &draft)?;
    let id = Client::created_id(data, DRAFT)?;
    let kept = replaces.and_then(|old| {
        let old = [old.to_owned()];
        check_set_reply(data, "notDestroyed", &old)
            .and_then(|()| check_confirmed(data, "destroyed", &old))
            .err()
            .map(|error| error.to_string())
    });
    Ok(SavedDraft { id, kept })
}

/// A draft as compose takes it back: the server's projection, with the
/// raw message's own `In-Reply-To` and `Bcc` where the projection has none.
fn resumed(email: &Email, raw: Option<&[u8]>) -> Composed {
    let header = |name: &str| raw.and_then(|raw| header_value(raw, name));
    let in_reply_to = email
        .in_reply_to
        .as_ref()
        .and_then(|ids| ids.first().cloned())
        .or_else(|| header("In-Reply-To"));
    let bcc = match email.bcc.clone().filter(|bcc| !bcc.is_empty()) {
        Some(bcc) => bcc,
        None => header("Bcc")
            .map(|bcc| draft_addresses(&bcc))
            .unwrap_or_default(),
    };
    Composed {
        from: String::new(),
        to: email.to.clone().unwrap_or_default(),
        cc: email.cc.clone().unwrap_or_default(),
        bcc,
        subject: email.subject.clone().unwrap_or_default(),
        body: email.text_body_value().unwrap_or_default().to_owned(),
        in_reply_to,
    }
}

/// The first `name` header of a raw message (case-insensitive, folded
/// lines unfolded), when it has one with a value.
fn header_value(raw: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let head = text
        .split("\r\n\r\n")
        .next()
        .and_then(|head| head.split("\n\n").next())
        .unwrap_or_default();
    let mut found: Option<String> = None;
    for line in head.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            if let Some(value) = found.as_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
            continue;
        }
        if found.is_some() {
            break;
        }
        if let Some((key, value)) = line.split_once(':')
            && key.trim().eq_ignore_ascii_case(name)
        {
            found = Some(value.trim().to_owned());
        }
    }
    found.filter(|value| !value.is_empty())
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
        assert!(
            mime.contains("To: \"Lee, Cy\" <cy@example.invalid>, Bob <bob@example.invalid>\r\n")
        );
    }

    #[test]
    fn the_body_follows_the_blank_line() {
        let mime = String::from_utf8(build_mime(&composed())).unwrap();
        let (_, body) = mime.split_once("\r\n\r\n").unwrap();
        assert_eq!(body, "Noon?");
    }

    #[test]
    fn a_split_keeps_quoted_separators_and_the_trailing_entry() {
        let text = "ann@example.invalid, \"Lee, Cy\" <cy@example.invalid>; bo";
        let split = split_addresses(text);
        let entries: Vec<&str> = split.entries.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(
            entries,
            [
                "ann@example.invalid",
                " \"Lee, Cy\" <cy@example.invalid>",
                " bo"
            ]
        );
        assert!(!split.open && !split.nested);
        // A trailing separator leaves an empty last entry.
        let split = split_addresses("ann@example.invalid,");
        assert_eq!(split.entries.last(), Some(&(20..20)));
        // A quote still open is reported, its comma not a separator.
        let split = split_addresses("\"Lee, C");
        assert_eq!((split.entries.len(), split.open), (1, true));
    }

    #[test]
    fn a_draft_keeps_entries_that_do_not_parse() {
        assert_eq!(
            draft_addresses("Bob <bob@example.invalid>, ann, ,"),
            vec![named("Bob", "bob@example.invalid"), bare("ann")]
        );
    }

    #[test]
    fn a_draft_carries_its_bcc_and_a_send_does_not() {
        let draft = String::from_utf8(build_draft_mime(&composed())).unwrap();
        assert!(draft.contains("Bcc: dave@example.invalid\r\n"), "{draft}");
        let sent = String::from_utf8(build_mime(&composed())).unwrap();
        assert!(!sent.contains("Bcc:"));
    }

    /// The exact request: one Email/set creating the draft in Drafts as
    /// `$draft $seen` and destroying the draft it replaces.
    #[test]
    fn a_draft_save_is_one_create_and_destroy() {
        assert_eq!(
            draft_set_args("A1", "B7", "M-drafts", Some("E-old")),
            json!({
                "accountId": "A1",
                "create": {
                    "draft": {
                        "blobId": "B7",
                        "mailboxIds": { "M-drafts": true },
                        "keywords": { "$draft": true, "$seen": true }
                    }
                },
                "destroy": ["E-old"]
            })
        );
        let first = draft_set_args("A1", "B7", "M-drafts", None);
        assert!(first.get("destroy").is_none(), "{first}");
    }

    #[test]
    fn a_draft_save_needs_its_create_confirmed() {
        let confirmed = json!({
            "created": { "draft": { "id": "E-new" } },
            "destroyed": ["E-old"]
        });
        assert_eq!(
            draft_outcome(&confirmed, Some("E-old")).unwrap(),
            SavedDraft {
                id: "E-new".into(),
                kept: None
            }
        );
        // maild drops a failed create silently: no id is no save.
        let dropped = json!({ "created": {}, "destroyed": ["E-old"] });
        assert!(draft_outcome(&dropped, Some("E-old")).is_err());
        let refused = json!({
            "notCreated": { "draft": { "type": "invalidProperties" } }
        });
        assert!(draft_outcome(&refused, None).is_err());
    }

    #[test]
    fn an_unconfirmed_destroy_keeps_the_new_draft_with_a_note() {
        for reply in [
            json!({ "created": { "draft": { "id": "E-new" } } }),
            json!({
                "created": { "draft": { "id": "E-new" } },
                "notDestroyed": { "E-old": { "type": "notFound" } }
            }),
        ] {
            let saved = draft_outcome(&reply, Some("E-old")).unwrap();
            assert_eq!(saved.id, "E-new");
            assert!(saved.kept.is_some(), "{reply}");
        }
    }

    #[test]
    fn a_resumed_draft_reads_its_own_headers_where_the_projection_has_none() {
        let email: Email = serde_json::from_value(json!({
            "id": "E1",
            "subject": "Lunch",
            "to": ["Ann <ann@example.invalid>"],
            "cc": [],
            "bcc": [],
            "inReplyTo": null,
            "textBody": [{ "partId": "1" }],
            "bodyValues": { "1": { "value": "Noon?" } }
        }))
        .unwrap();
        let raw = b"From: bob@example.invalid\r\nBcc: dave@example.invalid,\r\n Eve <eve@example.invalid>\r\nIn-Reply-To: <m1@example.invalid>\r\n\r\nNoon?";
        let draft = resumed(&email, Some(raw));
        assert_eq!(draft.to, vec![named("Ann", "ann@example.invalid")]);
        assert_eq!(
            draft.bcc,
            vec![
                bare("dave@example.invalid"),
                named("Eve", "eve@example.invalid")
            ]
        );
        assert_eq!(draft.in_reply_to.as_deref(), Some("<m1@example.invalid>"));
        assert_eq!((draft.subject.as_str(), draft.body.as_str()), ("Lunch", "Noon?"));
        // A header in the body is not a header.
        assert_eq!(header_value(b"A: 1\r\n\r\nBcc: x@example.invalid", "Bcc"), None);
    }
}
