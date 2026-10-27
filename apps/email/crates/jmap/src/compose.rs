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
//! The body is always `text/plain`: no HTML part is ever generated, by
//! design, for security (docs/docs/decisions/2026-10-27-email-sends-no-html.md).
//! Do not add a Markdown-to-HTML step or a `multipart/alternative` body.
//!
//! The Dioxus faults fixed here: Bcc is carried through; `In-Reply-To` is
//! wrapped exactly once (a JMAP `messageId` arrives already bracketed); and
//! every header input is stripped of CR/LF before it reaches the wire
//! (header injection).

use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use base64::Engine;
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
    /// Absolute file paths. Bytes are read when preparing an unsent message.
    pub attachments: Vec<PathBuf>,
    /// The message being replied to, as `Email.messageId` gives it
    /// (bracketed). Sent as `In-Reply-To`/`References`.
    pub in_reply_to: Option<String>,
    /// Ordered thread ancestors; falls back to In-Reply-To when empty.
    pub references: Vec<String>,
    /// The display name in From, when the sender identity has one. Empty:
    /// the From is the address alone, as before identities.
    pub from_name: String,
    /// The Reply-To address, when the sender identity sets one.
    pub reply_to: Option<String>,
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
/// `In-Reply-To` keeps exactly one pair of angle brackets; Bcc stays in
/// the submission envelope. File failures are returned to the caller;
/// the sender applies the server's upload limit to the finished bytes.
pub fn build_mime(composed: &Composed) -> Result<Vec<u8>> {
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
fn build_mime_with_id(composed: &Composed, message_id: &str) -> Result<Vec<u8>> {
    let parts = read_parts(&composed.attachments, None)?;
    Ok(render_mime(composed, message_id, &parts))
}

fn render_mime(composed: &Composed, message_id: &str, parts: &[String]) -> Vec<u8> {
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
    let from = format_address(&EmailAddress {
        name: Some(composed.from_name.clone()).filter(|name| !name.is_empty()),
        email: composed.from.clone(),
    });
    header(&mut mime, "From", &clean(&from));
    if let Some(reply_to) = &composed.reply_to {
        header(&mut mime, "Reply-To", &clean(reply_to));
    }
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
    }
    let references = if composed.references.is_empty() {
        composed.in_reply_to.iter().collect::<Vec<_>>()
    } else {
        composed.references.iter().collect::<Vec<_>>()
    };
    header(
        &mut mime,
        "References",
        &references
            .into_iter()
            .map(|id| one_wrap(&clean(id)))
            .collect::<Vec<_>>()
            .join(" "),
    );
    mime.push_str("MIME-Version: 1.0\r\n");
    if !parts.is_empty() {
        let text = text_part(&composed.body);
        let boundary = choose_boundary(&text, parts, || {
            format!("=_email_{}", uuid::Uuid::now_v7().simple())
        });
        mime.push_str(&format!(
            "Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n"
        ));
        for part in std::iter::once(&text).chain(parts) {
            mime.push_str(&format!("--{boundary}\r\n{part}\r\n"));
        }
        mime.push_str(&format!("--{boundary}--\r\n"));
        return mime.into_bytes();
    }
    mime.push_str(&text_part(&composed.body));
    mime.into_bytes()
}

fn text_part(body: &str) -> String {
    let mut mime = String::new();
    mime.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    mime.push_str("Content-Transfer-Encoding: 8bit\r\n");
    mime.push_str("\r\n");
    // RFC 5322 wants CRLF on every line; a text editor hands over bare LF.
    mime.push_str(&body.replace("\r\n", "\n").replace('\n', "\r\n"));
    mime
}

fn choose_boundary(text: &str, parts: &[String], mut candidate: impl FnMut() -> String) -> String {
    loop {
        let boundary = candidate();
        if !text.contains(&boundary) && parts.iter().all(|part| !part.contains(&boundary)) {
            return boundary;
        }
    }
}

/// The local limit applies only when the server advertises no limit.
pub const DEFAULT_UPLOAD_LIMIT: u64 = 25 * 1024 * 1024;

/// Pure check of the whole encoded message, including headers and boundaries.
pub fn check_message_size(size: u64, max_size_upload: Option<u64>) -> Result<()> {
    let limit = max_size_upload.unwrap_or(DEFAULT_UPLOAD_LIMIT);
    if size > limit {
        Err(Error::MessageTooLarge { size, limit })
    } else {
        Ok(())
    }
}

fn encoded_file_size(size: u64) -> u64 {
    let chars = size.saturating_add(2) / 3;
    let chars = chars.saturating_mul(4);
    chars.saturating_add(chars.div_ceil(76).saturating_mul(2))
}

fn unreadable(path: &Path, source: std::io::Error) -> Error {
    Error::AttachmentUnreadable {
        path: path.to_owned(),
        source,
    }
}

/// Metadata only: command dispatch must never open or read an attachment.
fn attachment_metadata(path: &Path) -> Result<std::fs::Metadata> {
    if !path.is_absolute() {
        return Err(Error::AttachmentRelative(path.to_owned()));
    }
    let metadata = std::fs::metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::AttachmentNotFile(path.to_owned())
        } else {
            unreadable(path, error)
        }
    })?;
    if !metadata.is_file() {
        return Err(Error::AttachmentNotFile(path.to_owned()));
    }
    Ok(metadata)
}

/// Follow symlinks, then check the opened file too. O_NONBLOCK prevents a
/// path replaced by a FIFO between stat and open from hanging the caller;
/// O_NOCTTY prevents a substituted terminal becoming the controlling tty.
fn open_attachment(path: &Path) -> Result<std::fs::File> {
    attachment_metadata(path)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .map_err(|error| unreadable(path, error))?;
    if !file
        .metadata()
        .map_err(|error| unreadable(path, error))?
        .is_file()
    {
        return Err(Error::AttachmentNotFile(path.to_owned()));
    }
    Ok(file)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn attachment_headers(name: &str, mime: &str) -> String {
    // Quoted ASCII fallback, with no header controls or unescaped specials.
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_control() {
                c
            } else {
                '_'
            }
        })
        .collect();
    let fallback = fallback.replace('\\', "\\\\").replace('"', "\\\"");
    let mut headers = format!(
        "Content-Type: {mime}\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"{fallback}\""
    );
    if !name.is_ascii() {
        let mut encoded = String::new();
        for byte in name.bytes() {
            if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
                encoded.push(char::from(byte));
            } else {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
        headers.push_str(&format!(";\r\n filename*=UTF-8''{encoded}"));
    }
    headers.push_str("\r\n\r\n");
    headers
}

/// Cheap command-time validation: stat paths and estimate encoded size.
/// Opening, reading and MIME database loading happen only on the send worker.
pub fn validate_attachments(composed: &Composed, max_size_upload: Option<u64>) -> Result<()> {
    let files = composed
        .attachments
        .iter()
        .map(|path| Ok((path.as_path(), attachment_metadata(path)?.len())))
        .collect::<Result<Vec<_>>>()?;
    check_message_size(estimated_message_size(composed, files), max_size_upload)
}

/// The same encoded-size estimate, using already inspected file sizes. Compose
/// can update its warning without file reads or loading the MIME database.
/// Reserve the RFC 6838 maximum media type length: 127 + '/' + 127 bytes.
pub fn estimated_message_size<'a>(
    composed: &Composed,
    files: impl IntoIterator<Item = (&'a Path, u64)>,
) -> u64 {
    let mut parts = Vec::new();
    let mut encoded = 0_u64;
    let padded_media_type = "x".repeat(255);
    for (path, size) in files {
        encoded = encoded.saturating_add(encoded_file_size(size));
        let name = file_name(path);
        parts.push(attachment_headers(&name, &padded_media_type));
    }
    (render_mime(composed, &new_message_id(&composed.from), &parts).len() as u64)
        .saturating_add(encoded)
}

fn mime_database() -> &'static mimedb::Database {
    static DB: OnceLock<mimedb::Database> = OnceLock::new();
    DB.get_or_init(mimedb::Database::load)
}

/// `limit` is absent for standalone builders, which impose no upload policy.
fn read_parts(paths: &[PathBuf], limit: Option<u64>) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let db = mime_database();
    let mut parts = Vec::new();
    let mut encoded = 0_u64;
    for path in paths {
        let file = open_attachment(path)?;
        let size = file
            .metadata()
            .map_err(|error| unreadable(path, error))?
            .len();
        // Reject a known oversized input before allocating its contents.
        if let Some(limit) = limit {
            check_message_size(encoded.saturating_add(encoded_file_size(size)), Some(limit))?;
        }
        let mut bytes = Vec::new();
        // Bound reads even when a file grows after metadata validation.
        file.take(limit.map_or(u64::MAX, |limit| limit.saturating_add(1)))
            .read_to_end(&mut bytes)
            .map_err(|error| unreadable(path, error))?;
        encoded = encoded.saturating_add(encoded_file_size(bytes.len() as u64));
        if let Some(limit) = limit {
            check_message_size(encoded, Some(limit))?;
        }
        let name = file_name(path);
        let mime = db
            .guess(Some(&name), Some(&bytes[..bytes.len().min(db.sniff_len())]))
            .mime;
        let mut part = attachment_headers(&name, &mime);
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        for line in data.as_bytes().chunks(76) {
            part.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
            part.push_str("\r\n");
        }
        parts.push(part);
    }
    Ok(parts)
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
    /// Compose fields and attachment paths only, so a confirmed earlier send
    /// can detect edits without reopening files. Absent in older saved drafts.
    pub compose_fingerprint: Option<String>,
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
    /// An earlier try had already been submitted: nothing new went out
    /// this time.
    pub earlier: bool,
    /// The message went, but moving it into Sent failed (it stays in
    /// Drafts on the server), and why.
    pub unfiled: Option<String>,
}

/// The JMAP calls the send recipe makes: the connected [`Client`], or a
/// test's fake server.
pub(crate) trait Wire {
    fn account(&self) -> &str;
    fn max_size_upload(&self) -> Option<u64> {
        None
    }
    async fn calls(&self, calls: &[Call]) -> Result<Vec<Value>>;
    async fn upload(&self, data: Vec<u8>, content_type: &str) -> Result<String>;
}

impl Wire for Client {
    fn max_size_upload(&self) -> Option<u64> {
        self.session().max_size_upload
    }
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
fn fingerprint(composed: &Composed, parts: &[String]) -> String {
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
    feed(&composed.from_name);
    feed(composed.reply_to.as_deref().unwrap_or(""));
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
    // Preserve fingerprints of older JSON drafts with no explicit references.
    if !composed.references.is_empty() {
        feed("references");
        feed(&composed.references.len().to_string());
        for reference in &composed.references {
            feed(reference);
        }
    }
    for (path, part) in composed.attachments.iter().zip(parts) {
        feed(&path.to_string_lossy());
        feed(part);
    }
    format!("{hash:016x}")
}

fn compose_fingerprint(composed: &Composed) -> String {
    fingerprint(composed, &vec![String::new(); composed.attachments.len()])
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
async fn create_draft<W: Wire>(wire: &W, mime: Vec<u8>, drafts: &str) -> Result<String> {
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
    // Resolve the server's outcome before touching local files or limits.
    let mut earlier = attempt.submitted;
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
                }
                Ok(None) => attempt.ambiguous = false,
            },
            None => attempt.ambiguous = false,
        }
    }
    if attempt.submitted {
        let current = compose_fingerprint(composed);
        let previous = attempt.compose_fingerprint.as_deref().or({
            // Before attachments, this was also the full fingerprint.
            if composed.attachments.is_empty() {
                attempt.fingerprint.as_deref()
            } else {
                // Legacy attachment attempts have no fields-only baseline.
                // Server confirmation still takes precedence over local I/O.
                None
            }
        });
        if previous.is_some_and(|previous| previous != current) {
            if let Some(email_id) = &attempt.email_id {
                let _ = file_sent(wire, email_id, &boxes.sent).await;
            }
            return (SendAttempt::default(), Err(Error::AlreadySent));
        }
    }
    if !attempt.submitted {
        let limit = wire.max_size_upload().unwrap_or(DEFAULT_UPLOAD_LIMIT);
        let paths = composed.attachments.clone();
        let parts = match tokio::task::spawn_blocking(move || read_parts(&paths, Some(limit))).await
        {
            Ok(Ok(parts)) => parts,
            Ok(Err(error)) => return (attempt, Err(error)),
            Err(error) => {
                return (
                    attempt,
                    Err(Error::Protocol(format!("attachment worker: {error}"))),
                );
            }
        };
        let print = fingerprint(composed, &parts);
        let changed = attempt.fingerprint.as_deref() != Some(print.as_str());
        // Render once when a new server draft is needed, check these exact
        // bytes before changing server state, and reuse them for the upload.
        let mime = if attempt.email_id.is_none() || changed {
            let message_id = if changed && attempt.email_id.is_some() {
                new_message_id(&composed.from)
            } else {
                attempt
                    .message_id
                    .clone()
                    .unwrap_or_else(|| new_message_id(&composed.from))
            };
            let mime = render_mime(composed, &message_id, &parts);
            if let Err(error) = check_message_size(mime.len() as u64, Some(limit)) {
                return (attempt, Err(error));
            }
            attempt.message_id = Some(message_id);
            Some(mime)
        } else {
            None
        };
        if let Some(stale) = attempt.email_id.clone().filter(|_| changed) {
            destroy_draft(wire, &stale).await;
            attempt.email_id = None;
            attempt.fingerprint = None;
        }
        // Read before anything is created: a failure here creates nothing.
        let identity = match identity_for(wire, &composed.from).await {
            Ok(identity) => identity,
            Err(error) => return (attempt, Err(error)),
        };
        let email_id = match attempt.email_id.clone() {
            Some(email_id) => email_id,
            None => {
                let mime = mime.expect("a new draft has MIME bytes");
                match create_draft(wire, mime, &boxes.drafts).await {
                    Ok(email_id) => {
                        attempt.email_id = Some(email_id.clone());
                        attempt.fingerprint = Some(print);
                        email_id
                    }
                    Err(error) => return (attempt, Err(error)),
                }
            }
        };
        attempt.compose_fingerprint = Some(compose_fingerprint(composed));
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

    #[test]
    fn no_attachments_keep_the_original_wire_bytes() {
        let mime =
            String::from_utf8(build_mime_with_id(&composed(), "fixed@example.invalid").unwrap())
                .unwrap();
        let date = mime
            .lines()
            .find(|line| line.starts_with("Date: "))
            .unwrap();
        assert_eq!(
            mime,
            format!(
                "From: bob@example.invalid\r\nTo: ann@example.invalid\r\nCc: carol@example.invalid\r\nSubject: Lunch\r\n{date}\r\nMessage-ID: <fixed@example.invalid>\r\nIn-Reply-To: <m1@example.invalid>\r\nReferences: <m1@example.invalid>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\nNoon?"
            )
        );
    }

    fn attached(name: &str, bytes: &[u8]) -> (tempfile::TempDir, Composed) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        let message = Composed {
            attachments: vec![path],
            ..composed()
        };
        (dir, message)
    }

    #[test]
    fn multipart_text_first_then_files_with_wrapped_base64_and_mime_types() {
        let bytes: Vec<u8> = (0..255).collect();
        let (dir, mut message) = attached("image.png", &bytes);
        let empty = dir.path().join("empty.txt");
        std::fs::write(&empty, []).unwrap();
        message.attachments.push(empty);
        let mime = String::from_utf8(build_mime(&message).unwrap()).unwrap();
        let boundary = mime
            .split("boundary=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let sections: Vec<&str> = mime.split(&format!("--{boundary}")).collect();
        assert_eq!(sections.len(), 5);
        assert!(sections[1].starts_with("\r\nContent-Type: text/plain; charset=utf-8\r\n"));
        assert!(sections[1].ends_with("\r\n\r\nNoon?\r\n"));
        assert!(sections[2].contains("Content-Type: image/png\r\n"));
        assert!(sections[2].contains("filename=\"image.png\""));
        assert!(sections[3].contains("filename=\"empty.txt\""));
        let data = sections[2].split_once("\r\n\r\n").unwrap().1.trim_end();
        let lines: Vec<&str> = data.split("\r\n").collect();
        assert!(lines[..lines.len() - 1].iter().all(|line| line.len() == 76));
        assert!(lines.last().unwrap().len() <= 76);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(lines.concat())
                .unwrap(),
            bytes
        );
        assert!(sections[1..4].iter().all(|part| !part.contains(boundary)));
        assert_eq!(sections[4], "--\r\n");
    }

    #[test]
    fn non_ascii_filename_uses_rfc2231_and_headers_escape_specials() {
        let (_dir, message) = attached("résumé \"2026\".txt", b"hello");
        let mime = String::from_utf8(build_mime(&message).unwrap()).unwrap();
        assert!(mime.contains("filename=\"r_sum_ \\\"2026\\\".txt\""));
        assert!(mime.contains("filename*=UTF-8''r%C3%A9sum%C3%A9%20%222026%22.txt"));
        let headers = attachment_headers("evil\r\nInjected: yes\\.txt", "text/plain");
        assert!(!headers.contains("\r\nInjected:"));
        assert!(headers.contains("yes\\\\.txt"));
    }

    #[test]
    fn boundary_candidates_are_checked_against_text_and_attachment_headers() {
        let mut candidates = ["text-collision", "file-collision", "safe"].into_iter();
        let boundary = choose_boundary(
            "text-collision",
            &["filename=file-collision".into()],
            || candidates.next().unwrap().into(),
        );
        assert_eq!(boundary, "safe");
    }

    #[test]
    fn encoded_limits_include_base64_wrapping_and_allow_the_exact_limit() {
        for size in [0, 1, 2, 3, 56, 57, 58, 114, 115] {
            let encoded = base64::engine::general_purpose::STANDARD.encode(vec![0; size]);
            assert_eq!(
                encoded_file_size(size as u64),
                (encoded.len() + encoded.len().div_ceil(76) * 2) as u64
            );
        }
        assert!(check_message_size(DEFAULT_UPLOAD_LIMIT, None).is_ok());
        assert!(matches!(
            check_message_size(DEFAULT_UPLOAD_LIMIT + 1, None),
            Err(Error::MessageTooLarge { .. })
        ));
        assert!(
            check_message_size(DEFAULT_UPLOAD_LIMIT + 1, Some(DEFAULT_UPLOAD_LIMIT + 1)).is_ok()
        );
        assert!(check_message_size(123, Some(123)).is_ok());
        assert!(check_message_size(124, Some(123)).is_err());
        assert!(check_message_size(1, Some(0)).is_err());
    }

    #[test]
    fn standalone_builders_do_not_impose_the_fallback_upload_limit() {
        let message = Composed {
            body: "x".repeat(DEFAULT_UPLOAD_LIMIT as usize + 1),
            ..composed()
        };
        assert!(build_mime(&message).unwrap().len() as u64 > DEFAULT_UPLOAD_LIMIT);

        let (_dir, message) = attached("large.bin", b"");
        // Base64 alone exceeds the fallback, even though the source does not.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&message.attachments[0])
            .unwrap()
            .set_len(DEFAULT_UPLOAD_LIMIT * 3 / 4 + 1)
            .unwrap();
        let mime = build_mime_with_id(&message, &new_message_id(&message.from)).unwrap();
        assert!(mime.len() as u64 > DEFAULT_UPLOAD_LIMIT);
        let fake = Fake::new(&[]);
        fake.server().limit = Some(mime.len() as u64);
        assert!(try_send(&fake, &message, SendAttempt::default()).1.is_ok());
        assert_eq!(fake.server().uploads[0].len(), mime.len());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn command_validation_never_opens_or_reads_attachment_files() {
        use std::os::fd::{FromRawFd, OwnedFd};
        use std::os::unix::ffi::OsStrExt;

        let (_dir, message) = attached("metadata.txt", b"contents");
        let path = std::ffi::CString::new(message.attachments[0].as_os_str().as_bytes()).unwrap();
        // SAFETY: flags are valid and the descriptor is owned below.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(fd >= 0);
        // SAFETY: fd is newly allocated and transferred to OwnedFd once.
        let _owned = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: path is NUL-terminated and fd remains open.
        let watch =
            unsafe { libc::inotify_add_watch(fd, path.as_ptr(), libc::IN_OPEN | libc::IN_ACCESS) };
        assert!(watch >= 0);
        validate_attachments(&message, None).unwrap();
        let mut events = [0_u8; 4096];
        // SAFETY: events is writable for the full buffer length.
        let read = unsafe { libc::read(fd, events.as_mut_ptr().cast(), events.len()) };
        assert_eq!(read, -1);
        assert_eq!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn attachment_preparation_yields_to_the_async_runtime() {
        use std::future::Future;
        use std::task::Poll;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let (_dir, message) = attached("worker.txt", b"contents");
        let fake = Fake::new(&[]);
        runtime.block_on(async {
            let (release, wait) = std::sync::mpsc::channel();
            // Hold the sole blocking worker. Preparation must wait for it,
            // while the current-thread runtime can still poll other work.
            let blocker = tokio::task::spawn_blocking(move || {
                wait.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            });
            let boxes = boxes();
            let mut sending = std::pin::pin!(send(&fake, &message, &boxes, SendAttempt::default()));
            std::future::poll_fn(|cx| {
                assert!(matches!(sending.as_mut().poll(cx), Poll::Pending));
                Poll::Ready(())
            })
            .await;
            assert!(fake.server().uploads.is_empty());
            release.send(()).unwrap();
            assert!(sending.await.1.is_ok());
            blocker.await.unwrap();
        });
    }

    #[test]
    fn an_ambiguous_send_with_a_vanished_file_reports_sent_earlier() {
        let (_dir, message) = attached("gone.txt", b"sent contents");
        let fake = Fake::new(&[Submit::AcceptThenLose]);
        let (attempt, result) = try_send(&fake, &message, SendAttempt::default());
        assert!(result.is_err() && attempt.ambiguous);
        std::fs::remove_file(&message.attachments[0]).unwrap();
        fake.server().limit = Some(0);
        let (attempt, result) = try_send(&fake, &message, attempt);
        assert!(result.unwrap().earlier);
        assert!(attempt.submitted && !attempt.ambiguous);
        assert_eq!(fake.count("EmailSubmission/query"), 1);
        assert_eq!(fake.count("EmailSubmission/set"), 1);
        assert_eq!(fake.server().uploads.len(), 1);
        assert_eq!(fake.in_box("M-sent"), 1);
    }

    #[test]
    fn an_ambiguous_send_ignores_changed_file_contents_and_a_lower_limit() {
        let (_dir, message) = attached("changed.txt", b"sent contents");
        let fake = Fake::new(&[Submit::AcceptThenLose]);
        let (attempt, _) = try_send(&fake, &message, SendAttempt::default());
        std::fs::write(&message.attachments[0], b"different contents").unwrap();
        fake.server().limit = Some(0);
        let (_, result) = try_send(&fake, &message, attempt);
        assert!(result.unwrap().earlier);
        assert_eq!(fake.server().uploads.len(), 1);
        assert_eq!(fake.count("EmailSubmission/set"), 1);
    }

    #[test]
    fn a_confirmed_send_is_filed_without_reopening_attachments() {
        let (_dir, message) = attached("sent.txt", b"contents");
        let fake = Fake::new(&[]);
        let (attempt, result) = try_send(&fake, &message, SendAttempt::default());
        assert!(result.is_ok() && attempt.submitted);
        std::fs::remove_file(&message.attachments[0]).unwrap();
        fake.server().limit = Some(0);
        let (_, result) = try_send(&fake, &message, attempt);
        assert!(result.unwrap().earlier);
        assert_eq!(fake.count("EmailSubmission/query"), 0);
        assert_eq!(fake.count("EmailSubmission/set"), 1);
        assert_eq!(fake.in_box("M-sent"), 1);
    }

    #[test]
    fn an_unresolved_send_returns_the_server_error_before_file_errors() {
        let (_dir, message) = attached("unknown.txt", b"contents");
        let fake = Fake::new(&[Submit::AcceptThenLose]);
        let (attempt, _) = try_send(&fake, &message, SendAttempt::default());
        std::fs::remove_file(&message.attachments[0]).unwrap();
        fake.server().no_query = true;
        let (attempt, result) = try_send(&fake, &message, attempt);
        assert!(matches!(result, Err(Error::Method { .. })));
        assert!(attempt.ambiguous);
        assert_eq!(fake.count("EmailSubmission/set"), 1);
    }

    #[test]
    fn edits_after_an_earlier_attachment_send_are_kept_without_reopening_files() {
        let (_dir, mut message) = attached("edited.txt", b"contents");
        let fake = Fake::new(&[Submit::AcceptThenLose]);
        let (attempt, _) = try_send(&fake, &message, SendAttempt::default());
        std::fs::remove_file(&message.attachments[0]).unwrap();
        message.body = "edited body".into();
        let (attempt, result) = try_send(&fake, &message, attempt);
        assert!(matches!(result, Err(Error::AlreadySent)));
        assert_eq!(attempt, SendAttempt::default());
        assert_eq!(fake.count("EmailSubmission/set"), 1);
        assert_eq!(fake.in_box("M-sent"), 1);
    }

    #[test]
    fn old_attempts_without_a_compose_fingerprint_still_reconcile() {
        let (_dir, message) = attached("old.txt", b"contents");
        let fake = Fake::new(&[Submit::AcceptThenLose]);
        let (attempt, _) = try_send(&fake, &message, SendAttempt::default());
        let mut saved = serde_json::to_value(attempt).unwrap();
        saved.as_object_mut().unwrap().remove("compose_fingerprint");
        let attempt = serde_json::from_value(saved).unwrap();
        std::fs::remove_file(&message.attachments[0]).unwrap();
        let (_, result) = try_send(&fake, &message, attempt);
        assert!(result.unwrap().earlier);
        assert_eq!(fake.count("EmailSubmission/set"), 1);
    }

    #[test]
    fn send_reads_current_bytes_and_replaces_a_refused_draft_after_file_edits() {
        let (_dir, message) = attached("note.txt", b"before");
        validate_attachments(&message, None).unwrap();
        std::fs::write(&message.attachments[0], b"after").unwrap();
        let fake = Fake::new(&[Submit::Refuse]);
        let (attempt, result) = try_send(&fake, &message, SendAttempt::default());
        assert!(result.is_err());
        assert!(
            String::from_utf8(fake.server().uploads[0].clone())
                .unwrap()
                .contains("YWZ0ZXI=\r\n")
        );
        std::fs::write(&message.attachments[0], b"again").unwrap();
        let (_, result) = try_send(&fake, &message, attempt);
        assert!(result.is_ok());
        assert_eq!(fake.server().uploads.len(), 2);
        assert_eq!(fake.server().emails.len(), 1);
    }

    #[test]
    fn send_rechecks_missing_files_and_exact_encoded_size_before_upload() {
        let (_dir, message) = attached("note.txt", b"hello");
        let fake = Fake::new(&[]);
        let parts = read_parts(&message.attachments, None).unwrap();
        let id = new_message_id(&message.from);
        let size = render_mime(&message, &id, &parts).len() as u64;
        fake.server().limit = Some(size - 1);
        assert!(matches!(
            try_send(&fake, &message, SendAttempt::default()).1,
            Err(Error::MessageTooLarge { .. })
        ));
        assert!(fake.server().uploads.is_empty());
        fake.server().limit = Some(size);
        assert!(try_send(&fake, &message, SendAttempt::default()).1.is_ok());
        std::fs::remove_file(&message.attachments[0]).unwrap();
        assert!(matches!(
            try_send(&fake, &message, SendAttempt::default()).1,
            Err(Error::AttachmentNotFile(_))
        ));
    }

    #[test]
    fn cached_size_estimate_bounds_real_mime_with_small_per_part_padding() {
        fn check(name: &str, bytes: &[u8], expected: Option<&str>) {
            let (_dir, message) = attached(name, bytes);
            let len = bytes.len() as u64;
            let size = estimated_message_size(&message, [(message.attachments[0].as_path(), len)]);
            let mime = build_mime(&message).unwrap();
            let exact = mime.len() as u64;
            assert!(size >= exact, "{name}: estimated {size}, exact {exact}");
            assert!(size - exact < 300, "{name}: too much padding");
            if let Some(expected) = expected {
                assert!(
                    String::from_utf8_lossy(&mime)
                        .contains(&format!("Content-Type: {expected}\r\n"))
                );
            }
            let parts = read_parts(&message.attachments, None).unwrap();
            let encoded = parts[0].split_once("\r\n\r\n").unwrap().1;
            assert_eq!(encoded_file_size(len), encoded.len() as u64);
            assert!(
                encoded
                    .split_terminator("\r\n")
                    .all(|line| line.len() <= 76)
            );
            assert!(encoded.is_empty() || encoded.ends_with("\r\n"));
            assert!(validate_attachments(&message, Some(size)).is_ok());
            assert!(matches!(
                validate_attachments(&message, Some(size - 1)),
                Err(Error::MessageTooLarge { .. })
            ));
            std::fs::remove_file(&message.attachments[0]).unwrap();
            assert_eq!(
                estimated_message_size(&message, [(message.attachments[0].as_path(), len)]),
                size
            );
        }

        // No extension: the worker must recognise the PNG signature from bytes.
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0";
        check("image", png, Some("image/png"));
        check(
            "report.docx",
            b"contents",
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        );
        check(
            "payload.unknown-extension-for-email",
            &[0, 255, 0, 255],
            Some("application/octet-stream"),
        );
        // Cross both the base64 76-column and final partial-line edges.
        for len in [0, 1, 2, 3, 56, 57, 58, 114, 115, 1024] {
            check("payload", &vec![b'x'; len], None);
        }
    }

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
            attachments: Vec::new(),
            in_reply_to: Some("<m1@example.invalid>".into()),
            references: Vec::new(),
            from_name: String::new(),
            reply_to: None,
        }
    }

    #[test]
    fn an_identity_sends_its_name_and_reply_to() {
        let identity = Composed {
            from: "ann@work.example.invalid".into(),
            from_name: "Ann, Work".into(),
            reply_to: Some("help@work.example.invalid".into()),
            ..composed()
        };
        let mime = String::from_utf8(build_mime(&identity).unwrap()).unwrap();
        assert!(
            mime.contains("From: \"Ann, Work\" <ann@work.example.invalid>\r\n"),
            "{mime}"
        );
        assert!(
            mime.contains("Reply-To: help@work.example.invalid\r\n"),
            "{mime}"
        );
        // Without a name or reply-to, the headers read as they did before.
        let plain = String::from_utf8(build_mime(&composed()).unwrap()).unwrap();
        assert!(!plain.contains("\r\nReply-To:"), "{plain}");
        assert!(!plain.contains("From: \""), "{plain}");
    }

    #[test]
    fn bcc_stays_out_of_the_bytes() {
        let mime = String::from_utf8(build_mime(&composed()).unwrap()).unwrap();
        assert!(
            !mime.contains("Bcc:"),
            "the bytes must not name Bcc recipients"
        );
        assert!(!mime.contains("dave@example.invalid"));
    }

    #[test]
    fn in_reply_to_is_wrapped_exactly_once() {
        let mime = String::from_utf8(build_mime(&composed()).unwrap()).unwrap();
        assert!(mime.contains("In-Reply-To: <m1@example.invalid>\r\n"));
        let bare = Composed {
            in_reply_to: Some("m2@example.invalid".into()),
            ..composed()
        };
        let mime = String::from_utf8(build_mime(&bare).unwrap()).unwrap();
        assert!(mime.contains("In-Reply-To: <m2@example.invalid>\r\n"));
    }

    #[test]
    fn header_injection_is_stripped() {
        let evil = Composed {
            subject: "Hi\r\nBcc: eve@example.invalid".into(),
            to: vec![bare("ann@example.invalid\r\nBcc: eve@example.invalid")],
            ..composed()
        };
        let mime = String::from_utf8(build_mime(&evil).unwrap()).unwrap();
        assert!(!mime.contains("\nBcc: eve"));
        // Each stripped control character leaves one space.
        assert!(mime.contains("Subject: Hi  Bcc: eve@example.invalid\r\n"));
    }

    #[test]
    fn references_are_ordered_sanitised_and_part_of_the_retry_fingerprint() {
        let draft = Composed {
            references: vec![
                "first@example.invalid".into(),
                "<second@example.invalid>".into(),
            ],
            reply_to: Some("bob@example.invalid\r\nBcc: eve@example.invalid".into()),
            ..composed()
        };
        let without_references = Composed {
            references: Vec::new(),
            ..draft.clone()
        };
        assert_ne!(
            compose_fingerprint(&draft),
            compose_fingerprint(&without_references)
        );
        let mime = String::from_utf8(build_mime(&draft).unwrap()).unwrap();
        assert!(mime.contains("References: <first@example.invalid> <second@example.invalid>\r\n"));
        assert!(mime.contains("Reply-To: bob@example.invalid  Bcc: eve@example.invalid\r\n"));
        assert!(!mime.contains("\nBcc:"));
        let injected = Composed {
            references: vec!["one\r\nBcc: eve@example.invalid".into()],
            ..draft
        };
        let mime = String::from_utf8(build_mime(&injected).unwrap()).unwrap();
        assert!(!mime.contains("\nBcc:"));
        assert!(mime.contains("References: <one  Bcc: eve@example.invalid>\r\n"));
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
        let mime = String::from_utf8(build_mime(&message).unwrap()).unwrap();
        assert!(
            mime.contains("To: \"Lee, Cy\" <cy@example.invalid>, Bob <bob@example.invalid>\r\n")
        );
    }

    #[test]
    fn the_body_follows_the_blank_line() {
        let mime = String::from_utf8(build_mime(&composed()).unwrap()).unwrap();
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
        uploads: Vec<Vec<u8>>,
        limit: Option<u64>,
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
        fn max_size_upload(&self) -> Option<u64> {
            self.server().limit
        }
        fn account(&self) -> &str {
            "A1"
        }

        async fn upload(&self, data: Vec<u8>, content_type: &str) -> Result<String> {
            let mut server = self.server();
            assert_eq!(content_type, "message/rfc822");
            server.uploads.push(data);
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

    /// The fake answers immediately; file preparation uses the blocking pool.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
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
        let first =
            String::from_utf8(build_mime_with_id(&message, "x1@example.invalid").unwrap()).unwrap();
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
