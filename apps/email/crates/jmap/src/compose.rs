// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Composing and sending: build the RFC 5322 message, upload it as a blob,
//! create the Email in Drafts (`$draft`), submit that id, and only once the
//! server has confirmed the submission move the Email into Sent. A failed
//! submission therefore never shows in Sent.
//!
//! A send is retried with the [`SendAttempt`] the last try returned: it
//! keeps the created Email's id, so a retry submits the same message
//! instead of creating a second copy. When the last submission's outcome
//! is unknown (the reply was lost: a timeout, a gateway error), the retry
//! first asks the server for a submission of that Email
//! (`EmailSubmission/query`) and never submits it again if one exists.
//!
//! The Dioxus faults fixed here: Bcc is carried through; `In-Reply-To` is
//! wrapped exactly once (a JMAP `messageId` arrives already bracketed); and
//! every header input is stripped of CR/LF before it reaches the wire
//! (header injection).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Call, Client};
use crate::error::{Error, Result};
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
    build_mime_with_id(composed, &new_message_id(&composed.from))
}

/// A fresh message id from the sender's domain, unbracketed; the angle
/// brackets belong to `one_wrap`, not the uuid.
fn new_message_id(from: &str) -> String {
    let domain = from.rsplit('@').next().unwrap_or("localhost");
    format!(
        "{}@{}",
        uuid::Uuid::now_v7().simple(),
        domain.replace(['\r', '\n'], " ")
    )
}

/// [`build_mime`] under a given `Message-ID`: a retried send keeps the id
/// of its first attempt, so a recipient can recognise a repeat.
fn build_mime_with_id(composed: &Composed, message_id: &str) -> Vec<u8> {
    let clean = |value: &str| value.replace(['\r', '\n'], " ");
    let address_list = |list: &[EmailAddress]| {
        list.iter()
            .map(|a| clean(&format_address(a)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let message_id = clean(message_id);
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
    /// the one whose address is `from`; there is no fallback. The Dioxus
    /// client hardcoded the id as "1"; it is read from the account instead.
    pub async fn identity_id(&self, from: &str) -> Result<String> {
        identity_for(self, from).await
    }

    /// Send [`Composed`] (see [`send`]): `attempt` is what the last try of
    /// this message returned ([`SendAttempt::default`] for a first try), and
    /// the attempt returned is the one to retry with.
    pub async fn send(
        &self,
        composed: &Composed,
        boxes: &SendBoxes,
        attempt: SendAttempt,
    ) -> (SendAttempt, Result<SendReport>) {
        send(self, composed, boxes, attempt).await
    }
}

/// The mailboxes a send files under: the message is created in `drafts`
/// and moved to `sent` once its submission is confirmed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SendBoxes {
    pub drafts: String,
    pub sent: String,
}

/// What one message's sends have done on the server so far, kept between
/// tries (and stored with the local draft) so a retry recognises what it
/// already did. Ids are the server's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SendAttempt {
    /// The `Message-ID` the message went out under, unbracketed.
    pub message_id: Option<String>,
    /// The Email created in Drafts.
    pub email_id: Option<String>,
    /// A fingerprint of the content that Email holds: an edited message
    /// is never submitted under the old Email.
    pub fingerprint: Option<String>,
    /// The confirmed EmailSubmission.
    pub submission_id: Option<String>,
    /// The server confirmed the submission: the message has gone.
    pub submitted: bool,
    /// The last submission failed in a way that may still have reached the
    /// server: the next try asks before it submits again.
    pub ambiguous: bool,
}

/// What a send that went did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendReport {
    /// An earlier, ambiguous try had in fact been submitted: nothing new
    /// went out this time.
    pub earlier: bool,
    /// The message went, but moving it into Sent failed (it stays in
    /// Drafts on the server), and why.
    pub unfiled: Option<String>,
}

/// The JMAP calls the send recipe makes: the connected [`Client`], or a
/// test's fake server.
pub(crate) trait Wire {
    fn account(&self) -> &str;
    async fn calls(&self, calls: &[Call]) -> Result<Vec<Value>>;
    async fn upload(&self, data: Vec<u8>, content_type: &str) -> Result<String>;
}

impl Wire for Client {
    fn account(&self) -> &str {
        self.account_id()
    }

    async fn calls(&self, calls: &[Call]) -> Result<Vec<Value>> {
        self.call(calls).await
    }

    async fn upload(&self, data: Vec<u8>, content_type: &str) -> Result<String> {
        self.upload_blob(data, content_type).await
    }
}

/// One method call's reply arguments.
async fn one<W: Wire>(wire: &W, method: &str, args: Value) -> Result<Value> {
    wire.calls(&[Call(method.into(), args, "s0".into())])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Protocol(format!("{method} reply is empty")))
}

/// A stable fingerprint of what is sent (FNV-1a over every field, each
/// one length-prefixed), the same in every build.
fn fingerprint(composed: &Composed) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |text: &str| {
        for byte in (text.len() as u64)
            .to_le_bytes()
            .iter()
            .chain(text.as_bytes())
        {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(&composed.from);
    for list in [&composed.to, &composed.cc, &composed.bcc] {
        feed(&list.len().to_string());
        for address in list {
            feed(address.name.as_deref().unwrap_or(""));
            feed(&address.email);
        }
    }
    feed(&composed.subject);
    feed(&composed.body);
    feed(composed.in_reply_to.as_deref().unwrap_or(""));
    format!("{hash:016x}")
}

/// The submission identity to send `from` with (see [`Client::identity_id`]).
async fn identity_for<W: Wire>(wire: &W, from: &str) -> Result<String> {
    let data = one(wire, "Identity/get", json!({ "accountId": wire.account() })).await?;
    let list = data
        .get("list")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Protocol("Identity/get has no list".into()))?;
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
        .ok_or_else(|| Error::Protocol(format!("no submission identity for {from}")))
}

/// The submission the server holds for `email_id`, if any.
async fn submission_of<W: Wire>(wire: &W, email_id: &str) -> Result<Option<String>> {
    let data = one(
        wire,
        "EmailSubmission/query",
        json!({ "accountId": wire.account(), "filter": { "emailIds": [email_id] } }),
    )
    .await?;
    let ids = data
        .get("ids")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Protocol("EmailSubmission/query has no ids".into()))?;
    Ok(ids.first().and_then(Value::as_str).map(str::to_owned))
}

/// Create the message in Drafts; the new Email's id.
async fn create_draft<W: Wire>(
    wire: &W,
    composed: &Composed,
    message_id: &str,
    drafts: &str,
) -> Result<String> {
    let mime = build_mime_with_id(composed, message_id);
    let blob_id = wire.upload(mime, "message/rfc822").await?;
    // A top-level `blobId` on `Email/set` create is maild's extension:
    // RFC 8621 builds an Email from uploaded MIME with `Email/import`
    // instead, so a strict server refuses this. Sending requires maild;
    // `Email/import` is future work.
    let data = one(
        wire,
        "Email/set",
        json!({
            "accountId": wire.account(),
            "create": {
                "out1": {
                    "blobId": blob_id,
                    "mailboxIds": { drafts: true },
                    "keywords": { "$draft": true, "$seen": true }
                }
            }
        }),
    )
    .await?;
    if let Some(error) = Client::set_error(&data, "notCreated", "out1") {
        return Err(error);
    }
    Client::created_id(&data, "out1")
}

/// Submit `email_id`; the submission's id. The envelope names every
/// recipient, Bcc included, so Bcc people receive the message without
/// appearing in its bytes.
async fn submit<W: Wire>(
    wire: &W,
    composed: &Composed,
    email_id: &str,
    identity_id: &str,
) -> Result<String> {
    let rcpt_to: Vec<Value> = composed
        .to
        .iter()
        .chain(&composed.cc)
        .chain(&composed.bcc)
        .map(|address| json!({ "email": address.email }))
        .collect();
    let data = one(
        wire,
        "EmailSubmission/set",
        json!({
            "accountId": wire.account(),
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
        }),
    )
    .await?;
    if let Some(error) = Client::set_error(&data, "notCreated", "sub1") {
        return Err(error);
    }
    Client::created_id(&data, "sub1")
}

/// Move a submitted Email from Drafts into Sent, no longer a draft.
async fn file_sent<W: Wire>(wire: &W, email_id: &str, sent: &str) -> Result<()> {
    let data = one(
        wire,
        "Email/set",
        json!({
            "accountId": wire.account(),
            "update": {
                email_id: {
                    "mailboxIds": { sent: true },
                    "keywords": { "$seen": true }
                }
            }
        }),
    )
    .await?;
    crate::client::check_set_reply(&data, "notUpdated", &[email_id.to_owned()])
}

/// Destroy a server draft that no longer holds the message (best effort:
/// a leftover draft is harmless, a second send is not).
async fn destroy_draft<W: Wire>(wire: &W, email_id: &str) {
    let _ = one(
        wire,
        "Email/set",
        json!({ "accountId": wire.account(), "destroy": [email_id] }),
    )
    .await;
}

/// The send recipe, resumable from `attempt`:
///
/// 1. An earlier try whose submission may have reached the server is
///    reconciled first: the server is asked for a submission of its Email,
///    and a message it already holds is never submitted again. If the
///    server cannot say, nothing is sent and the attempt stays ambiguous.
/// 2. Unless already submitted: the identity is read; the Email from an
///    earlier try is reused when it holds this content (an edited message
///    replaces it); otherwise the message is uploaded and created in
///    Drafts. Then it is submitted.
/// 3. Once the submission is confirmed the Email moves to Sent. A failure
///    there is reported, but the message has gone.
///
/// The attempt returned is the one to retry with, whatever the outcome.
pub(crate) async fn send<W: Wire>(
    wire: &W,
    composed: &Composed,
    boxes: &SendBoxes,
    mut attempt: SendAttempt,
) -> (SendAttempt, Result<SendReport>) {
    let print = fingerprint(composed);
    let mut earlier = false;
    if attempt.ambiguous && !attempt.submitted {
        match attempt.email_id.clone() {
            Some(email_id) => match submission_of(wire, &email_id).await {
                // The server cannot say: sending again could duplicate.
                Err(error) => return (attempt, Err(error)),
                Ok(Some(submission)) => {
                    attempt.submission_id = Some(submission);
                    attempt.submitted = true;
                    attempt.ambiguous = false;
                    earlier = true;
                    if attempt.fingerprint.as_deref() != Some(print.as_str()) {
                        // What went is the earlier content: file it, and
                        // leave the edited message for the person to send
                        // as a new one.
                        let _ = file_sent(wire, &email_id, &boxes.sent).await;
                        return (SendAttempt::default(), Err(Error::AlreadySent));
                    }
                }
                Ok(None) => attempt.ambiguous = false,
            },
            None => attempt.ambiguous = false,
        }
    }
    if !attempt.submitted {
        if let Some(stale) = attempt
            .email_id
            .clone()
            .filter(|_| attempt.fingerprint.as_deref() != Some(print.as_str()))
        {
            destroy_draft(wire, &stale).await;
            attempt.email_id = None;
            attempt.fingerprint = None;
            attempt.message_id = None;
        }
        // Read before anything is created: a failure here creates nothing.
        let identity = match identity_for(wire, &composed.from).await {
            Ok(identity) => identity,
            Err(error) => return (attempt, Err(error)),
        };
        let email_id = match attempt.email_id.clone() {
            Some(email_id) => email_id,
            None => {
                let message_id = attempt
                    .message_id
                    .get_or_insert_with(|| new_message_id(&composed.from))
                    .clone();
                match create_draft(wire, composed, &message_id, &boxes.drafts).await {
                    Ok(email_id) => {
                        attempt.email_id = Some(email_id.clone());
                        attempt.fingerprint = Some(print.clone());
                        email_id
                    }
                    Err(error) => return (attempt, Err(error)),
                }
            }
        };
        match submit(wire, composed, &email_id, &identity).await {
            Ok(submission) => {
                attempt.submission_id = Some(submission);
                attempt.submitted = true;
                attempt.ambiguous = false;
            }
            Err(error) => {
                attempt.ambiguous = error.ambiguous();
                // The server no longer has a usable Email under that id:
                // the next try creates the message again.
                if let Error::Method { error_type, .. } = &error
                    && (error_type == "invalidEmail" || error_type == "notFound")
                {
                    attempt.email_id = None;
                    attempt.fingerprint = None;
                }
                return (attempt, Err(error));
            }
        }
    }
    let unfiled = match &attempt.email_id {
        Some(email_id) => file_sent(wire, email_id, &boxes.sent)
            .await
            .err()
            .map(|error| error.to_string()),
        None => Some("the sent message's id is unknown".into()),
    };
    (attempt, Ok(SendReport { earlier, unfiled }))
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

    // The send recipe against a fake server, with no network.

    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// How the fake answers the next submission.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Submit {
        Accept,
        /// Refused with a stated error: nothing was submitted.
        Refuse,
        /// Accepted, but the reply is lost (a gateway timeout).
        AcceptThenLose,
        /// Not accepted, and the reply is lost all the same.
        LoseBeforeAccept,
    }

    #[derive(Default)]
    struct Server {
        /// Email id -> its mailbox ids.
        emails: BTreeMap<String, Vec<String>>,
        /// (submission id, email id).
        submissions: Vec<(String, String)>,
        /// How each coming submission is answered, in order; then Accept.
        plan: Vec<Submit>,
        /// The submission query is not supported.
        no_query: bool,
        /// Every method called, in order.
        log: Vec<String>,
        next: u32,
    }

    struct Fake(Mutex<Server>);

    impl Fake {
        fn new(plan: &[Submit]) -> Self {
            Fake(Mutex::new(Server {
                plan: plan.to_vec(),
                ..Server::default()
            }))
        }

        fn server(&self) -> std::sync::MutexGuard<'_, Server> {
            self.0.lock().unwrap()
        }

        fn in_box(&self, mailbox: &str) -> usize {
            let server = self.server();
            server
                .emails
                .values()
                .filter(|boxes| boxes.iter().any(|b| b == mailbox))
                .count()
        }

        fn count(&self, method: &str) -> usize {
            self.server().log.iter().filter(|m| *m == method).count()
        }
    }

    impl Wire for Fake {
        fn account(&self) -> &str {
            "A1"
        }

        async fn upload(&self, _data: Vec<u8>, _content_type: &str) -> Result<String> {
            let mut server = self.server();
            server.next += 1;
            Ok(format!("B{}", server.next))
        }

        async fn calls(&self, calls: &[Call]) -> Result<Vec<Value>> {
            let mut server = self.server();
            let Call(method, args, _) = &calls[0];
            server.log.push(method.clone());
            let boxes = |value: &Value| -> Vec<String> {
                value
                    .as_object()
                    .map(|map| map.keys().cloned().collect())
                    .unwrap_or_default()
            };
            let reply = match method.as_str() {
                "Identity/get" => json!({
                    "list": [{ "id": "I1", "email": "bob@example.invalid" }]
                }),
                "Email/set" if args.get("create").is_some() => {
                    server.next += 1;
                    let id = format!("E{}", server.next);
                    let create = &args["create"]["out1"];
                    assert_eq!(create["keywords"]["$draft"], json!(true));
                    let mailboxes = boxes(&create["mailboxIds"]);
                    server.emails.insert(id.clone(), mailboxes);
                    json!({ "created": { "out1": { "id": id } } })
                }
                "Email/set" if args.get("update").is_some() => {
                    let update = args["update"].as_object().unwrap();
                    let (id, patch) = update.iter().next().unwrap();
                    let mailboxes = boxes(&patch["mailboxIds"]);
                    match server.emails.get_mut(id) {
                        Some(entry) => {
                            *entry = mailboxes;
                            json!({ "updated": { id.clone(): null } })
                        }
                        None => json!({ "notUpdated": { id.clone(): { "type": "notFound" } } }),
                    }
                }
                "Email/set" => {
                    for id in args["destroy"].as_array().unwrap() {
                        server.emails.remove(id.as_str().unwrap());
                    }
                    json!({ "destroyed": args["destroy"] })
                }
                "EmailSubmission/set" => {
                    let email = args["create"]["sub1"]["emailId"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    assert!(server.emails.contains_key(&email), "submits a real email");
                    let plan = if server.plan.is_empty() {
                        Submit::Accept
                    } else {
                        server.plan.remove(0)
                    };
                    if plan == Submit::Refuse {
                        return Ok(vec![json!({
                            "notCreated": { "sub1": { "type": "forbiddenToSend" } }
                        })]);
                    }
                    if plan == Submit::LoseBeforeAccept {
                        return Err(Error::Status(reqwest::StatusCode::GATEWAY_TIMEOUT));
                    }
                    server.next += 1;
                    let id = format!("S{}", server.next);
                    server.submissions.push((id.clone(), email));
                    if plan == Submit::AcceptThenLose {
                        return Err(Error::Status(reqwest::StatusCode::GATEWAY_TIMEOUT));
                    }
                    json!({ "created": { "sub1": { "id": id } } })
                }
                "EmailSubmission/query" => {
                    if server.no_query {
                        return Err(Error::method("unknownMethod", None));
                    }
                    let wanted = args["filter"]["emailIds"][0].as_str().unwrap();
                    let ids: Vec<&str> = server
                        .submissions
                        .iter()
                        .filter(|(_, email)| email == wanted)
                        .map(|(id, _)| id.as_str())
                        .collect();
                    json!({ "ids": ids })
                }
                other => panic!("the fake does not answer {other}"),
            };
            Ok(vec![reply])
        }
    }

    /// Run a future that never waits (the fake answers at once).
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(output) => output,
            std::task::Poll::Pending => panic!("the fake server never waits"),
        }
    }

    fn boxes() -> SendBoxes {
        SendBoxes {
            drafts: "M-drafts".into(),
            sent: "M-sent".into(),
        }
    }

    fn try_send(
        fake: &Fake,
        message: &Composed,
        attempt: SendAttempt,
    ) -> (SendAttempt, Result<SendReport>) {
        block_on(send(fake, message, &boxes(), attempt))
    }

    #[test]
    fn a_send_is_created_in_drafts_and_filed_in_sent_after_submission() {
        let fake = Fake::new(&[]);
        let (attempt, result) = try_send(&fake, &composed(), SendAttempt::default());
        assert_eq!(result.unwrap(), SendReport::default());
        assert!(attempt.submitted && !attempt.ambiguous);
        assert_eq!(fake.in_box("M-sent"), 1);
        assert_eq!(fake.in_box("M-drafts"), 0);
        // Created, then submitted, then moved: in that order.
        let log = fake.server().log.clone();
        let at = |m: &str| log.iter().position(|x| x == m).unwrap();
        assert!(at("EmailSubmission/set") > at("Email/set"));
        assert_eq!(log.last().map(String::as_str), Some("Email/set"));
    }

    #[test]
    fn a_failed_submission_does_not_appear_in_sent() {
        let fake = Fake::new(&[Submit::Refuse]);
        let (attempt, result) = try_send(&fake, &composed(), SendAttempt::default());
        assert!(matches!(result, Err(Error::Method { .. })));
        assert_eq!(fake.in_box("M-sent"), 0, "nothing in Sent");
        assert_eq!(fake.in_box("M-drafts"), 1, "the message waits in Drafts");
        assert!(attempt.email_id.is_some() && !attempt.submitted);
        assert!(!attempt.ambiguous, "a stated refusal is not ambiguous");
    }

    #[test]
    fn a_retry_after_a_failed_submission_does_not_duplicate() {
        let fake = Fake::new(&[Submit::Refuse]);
        let message = composed();
        let (attempt, result) = try_send(&fake, &message, SendAttempt::default());
        assert!(result.is_err());
        let first = attempt.email_id.clone();
        let (attempt, result) = try_send(&fake, &message, attempt);
        assert_eq!(result.unwrap(), SendReport::default());
        assert_eq!(attempt.email_id, first, "the same Email was submitted");
        assert_eq!(fake.count("EmailSubmission/set"), 2);
        assert_eq!(fake.server().submissions.len(), 1);
        assert_eq!(fake.server().emails.len(), 1, "one copy on the server");
        assert_eq!(fake.in_box("M-sent"), 1);
    }

    #[test]
    fn an_ambiguous_failure_is_reconciled_before_retry() {
        let fake = Fake::new(&[Submit::AcceptThenLose]);
        let message = composed();
        let (attempt, result) = try_send(&fake, &message, SendAttempt::default());
        assert!(result.is_err());
        assert!(attempt.ambiguous && !attempt.submitted);
        assert_eq!(fake.in_box("M-sent"), 0, "not filed while unconfirmed");
        let (attempt, result) = try_send(&fake, &message, attempt);
        let report = result.unwrap();
        assert!(report.earlier, "the earlier try had gone");
        assert!(attempt.submitted);
        assert_eq!(fake.count("EmailSubmission/query"), 1);
        assert_eq!(
            fake.count("EmailSubmission/set"),
            1,
            "never submitted twice"
        );
        assert_eq!(fake.server().submissions.len(), 1);
        assert_eq!(fake.in_box("M-sent"), 1);
    }

    #[test]
    fn an_ambiguous_failure_that_never_landed_is_submitted_once_more() {
        let fake = Fake::new(&[Submit::LoseBeforeAccept]);
        let message = composed();
        let (attempt, _) = try_send(&fake, &message, SendAttempt::default());
        assert!(attempt.ambiguous);
        let (attempt, result) = try_send(&fake, &message, attempt);
        assert!(!result.unwrap().earlier);
        assert!(attempt.submitted);
        assert_eq!(fake.count("EmailSubmission/query"), 1);
        assert_eq!(fake.server().submissions.len(), 1);
        assert_eq!(fake.server().emails.len(), 1);
    }

    #[test]
    fn an_ambiguity_the_server_cannot_settle_is_never_resent() {
        let fake = Fake::new(&[Submit::AcceptThenLose]);
        let message = composed();
        let (attempt, _) = try_send(&fake, &message, SendAttempt::default());
        fake.server().no_query = true;
        let (attempt, result) = try_send(&fake, &message, attempt);
        assert!(result.is_err());
        assert!(attempt.ambiguous, "still unknown");
        assert_eq!(fake.count("EmailSubmission/set"), 1, "nothing resent");
    }

    #[test]
    fn an_edited_message_replaces_its_unsent_server_copy() {
        let fake = Fake::new(&[Submit::Refuse]);
        let (attempt, _) = try_send(&fake, &composed(), SendAttempt::default());
        let edited = Composed {
            body: "One o'clock?".into(),
            ..composed()
        };
        let (attempt, result) = try_send(&fake, &edited, attempt);
        assert!(result.is_ok());
        assert!(attempt.submitted);
        assert_eq!(fake.server().emails.len(), 1, "the stale copy is gone");
        assert_eq!(fake.in_box("M-sent"), 1);
    }

    #[test]
    fn an_edit_after_an_earlier_try_went_is_not_sent_as_well() {
        let fake = Fake::new(&[Submit::AcceptThenLose]);
        let (attempt, _) = try_send(&fake, &composed(), SendAttempt::default());
        let edited = Composed {
            body: "One o'clock?".into(),
            ..composed()
        };
        let (attempt, result) = try_send(&fake, &edited, attempt);
        assert!(matches!(result, Err(Error::AlreadySent)));
        assert_eq!(
            attempt,
            SendAttempt::default(),
            "a later send starts afresh"
        );
        assert_eq!(fake.server().submissions.len(), 1);
        assert_eq!(fake.in_box("M-sent"), 1, "what went is filed");
    }

    #[test]
    fn a_retry_keeps_the_first_message_id() {
        let message = composed();
        let first = String::from_utf8(build_mime_with_id(&message, "x1@example.invalid")).unwrap();
        assert!(first.contains("Message-ID: <x1@example.invalid>\r\n"));
        let fake = Fake::new(&[Submit::Refuse]);
        let (attempt, _) = try_send(&fake, &message, SendAttempt::default());
        let id = attempt.message_id.clone().expect("an id was chosen");
        let (attempt, _) = try_send(&fake, &message, attempt);
        assert_eq!(attempt.message_id, Some(id));
    }

    #[test]
    fn lost_replies_are_ambiguous_and_stated_refusals_are_not() {
        assert!(Error::Status(reqwest::StatusCode::GATEWAY_TIMEOUT).ambiguous());
        assert!(Error::Protocol("no id".into()).ambiguous());
        assert!(!Error::Status(reqwest::StatusCode::BAD_REQUEST).ambiguous());
        assert!(!Error::method("forbiddenToSend", None).ambiguous());
        assert!(!Error::Unauthorized("no".into()).ambiguous());
    }
}
