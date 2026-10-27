// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Inert mail documents using the ABP framing, never live Bus requests.
//!
//! See the [manual](https://mixos.dev/maildoc/) for the format, normalisation
//! and the canonical document requirements of [`write`]. No file is read and
//! no message is sent by this library. Email checks identities and validates
//! real mail addresses at send time.

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;

use bus::wire::{self, BusMessage};

/// The broker relay frame cap, including fences, headers and body in UTF-8.
pub const MAX_DOCUMENT_BYTES: usize = wire::WS_MAX_FRAME_BYTES;

const HEADERS: &[&str] = &[
    "mail-to",
    "mail-cc",
    "mail-bcc",
    "mail-from",
    "mail-reply-to",
    "mail-subject",
    "mail-in-reply-to",
    "mail-references",
    "mail-attachments",
];

/// A message's portable content, without account, identity or send state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailDocument {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub from: Option<String>,
    pub reply_to: Option<String>,
    pub subject: String,
    pub in_reply_to: Option<String>,
    /// Whitespace-separated message IDs on disk, kept in order.
    pub references: Vec<String>,
    /// Absolute UTF-8 paths only; their contents are never read here.
    pub attachments: Vec<PathBuf>,
    pub body: String,
}

/// A document refused before Email considers sending it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    TooLarge { bytes: usize },
    Crlf,
    Framing(String),
    HeaderNotAllowed(String),
    MalformedHeader(String),
    DuplicateHeader(String),
    EmptyValue(String),
    InvalidValue { header: String, reason: String },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes } => write!(
                f,
                "mail document is {bytes} bytes; maximum is {MAX_DOCUMENT_BYTES} bytes (8 MiB)"
            ),
            Self::Crlf => f.write_str("mail document uses CRLF; use LF and an opening '---\\n' fence"),
            Self::Framing(reason) => write!(f, "invalid mail document framing: {reason}"),
            Self::HeaderNotAllowed(name) => write!(f, "header not allowed in a mail document: {name}"),
            Self::MalformedHeader(line) => write!(f, "malformed mail document header: {line:?}"),
            Self::DuplicateHeader(name) => write!(f, "duplicate mail document header: {name}"),
            Self::EmptyValue(name) => write!(f, "empty {name}; omit empty headers"),
            Self::InvalidValue { header, reason } => write!(f, "invalid {header}: {reason}"),
        }
    }
}

impl std::error::Error for Refusal {}

fn invalid(header: &str, reason: impl Into<String>) -> Refusal {
    Refusal::InvalidValue {
        header: header.to_owned(),
        reason: reason.into(),
    }
}

fn single_line(header: &str, value: &str) -> Result<(), Refusal> {
    if value.contains(['\r', '\n']) {
        return Err(invalid(header, "CR or LF in a value"));
    }
    Ok(())
}

fn address(header: &str, value: &str) -> Result<(), Refusal> {
    if value.trim().is_empty() || !value.contains('@') {
        return Err(invalid(header, "address must be non-empty and contain @"));
    }
    Ok(())
}

fn addresses(header: &str, value: &str) -> Result<Vec<String>, Refusal> {
    value
        .split(',')
        .map(|part| {
            let part = part.trim();
            address(header, part)?;
            Ok(part.to_owned())
        })
        .collect()
}

/// Parse with [`wire::parse_lenient`], then enforce the mail header allowlist.
///
/// Empty values (including JSON `""`) and malformed or duplicate header lines
/// are refused. Unquoted values are trimmed by the wire parser; JSON strings
/// preserve significant spaces. Address entries are trimmed, references split
/// on whitespace, and the body loses all trailing Unicode whitespace.
pub fn parse(text: &str) -> Result<MailDocument, Refusal> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(Refusal::TooLarge { bytes: text.len() });
    }
    if text.contains("\r\n") {
        return Err(Refusal::Crlf);
    }
    let content = text
        .strip_prefix("---\n")
        .ok_or_else(|| Refusal::Framing("opening fence must be '---\\n'".into()))?;

    // Check the original lines too: the lenient parser skips malformed lines
    // and collapses duplicate keys. Neither may hide a forbidden header/value.
    let mut seen = BTreeSet::new();
    let mut closed = false;
    for line in content.split('\n') {
        if line == "---" {
            closed = true;
            break;
        }
        if line.is_empty() {
            continue;
        }
        let name = line.split_once(':').map_or(line, |(name, _)| name).trim();
        if !HEADERS.contains(&name) {
            return Err(Refusal::HeaderNotAllowed(name.to_owned()));
        }
        let (key, value) = line
            .split_once(": ")
            .ok_or_else(|| Refusal::MalformedHeader(line.to_owned()))?;
        if key != name {
            return Err(Refusal::MalformedHeader(line.to_owned()));
        }
        single_line(name, value)?;
        if !seen.insert(name) {
            return Err(Refusal::DuplicateHeader(name.to_owned()));
        }
    }
    if !closed {
        return Err(Refusal::Framing("missing closing fence".into()));
    }

    // wire::parse_lenient needs a newline before the closing fence, even for
    // zero headers. Supply that blank line for the compact empty-block form.
    let expanded;
    let input = if content.starts_with("---\n") || content == "---" {
        expanded = format!("---\n\n{content}");
        expanded.as_str()
    } else {
        text
    };
    let (message, report) = wire::parse_lenient(input)
        .map_err(|err| Refusal::Framing(err.to_string()))?;
    if let Some((_, line)) = report.skipped_lines.first() {
        return Err(Refusal::MalformedHeader(line.clone()));
    }
    let mut document = MailDocument {
        body: message.body,
        ..MailDocument::default()
    };
    for (name, raw) in message.headers {
        if raw.is_empty() {
            return Err(Refusal::EmptyValue(name));
        }
        if name == "mail-attachments" {
            let paths: Vec<String> = serde_json::from_str(&raw)
                .map_err(|err| invalid(&name, format!("expected a JSON array of paths: {err}")))?;
            for path in paths {
                single_line(&name, &path)?;
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(invalid(&name, "attachment path must be absolute"));
                }
                document.attachments.push(path);
            }
            continue;
        }
        // json_parse_errors is advisory for ordinary mail text: [PATCH] is
        // legal as written. We decode leading-quote JSON strings ourselves;
        // attachments above must always be an actual, well-formed JSON array.
        let value: String = if raw.starts_with('"') {
            serde_json::from_str(&raw)
                .map_err(|err| invalid(&name, format!("expected a JSON string: {err}")))?
        } else {
            raw
        };
        single_line(&name, &value)?;
        if value.is_empty() {
            return Err(Refusal::EmptyValue(name));
        }
        match name.as_str() {
            "mail-to" => document.to = addresses(&name, &value)?,
            "mail-cc" => document.cc = addresses(&name, &value)?,
            "mail-bcc" => document.bcc = addresses(&name, &value)?,
            "mail-from" | "mail-reply-to" => {
                address(&name, &value)?;
                if name == "mail-from" {
                    document.from = Some(value);
                } else {
                    document.reply_to = Some(value);
                }
            }
            "mail-subject" => document.subject = value,
            "mail-in-reply-to" => document.in_reply_to = Some(value),
            "mail-references" => {
                document.references = value.split_whitespace().map(str::to_owned).collect();
            }
            _ => return Err(Refusal::HeaderNotAllowed(name)),
        }
    }
    Ok(document)
}

fn set(message: &mut BusMessage, name: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    let encoded;
    let value = if value.starts_with(['[', '{', '"']) || value.trim() != value {
        encoded = serde_json::to_string(value).expect("strings are JSON serialisable");
        &encoded
    } else {
        value
    };
    message.set(name, value);
}

/// Write sorted headers with LF framing, omitting empty/absent values.
/// The body is appended exactly as given, without adding a newline.
///
/// For `parse(write(d)) == d`, `d` must be canonical: no header value contains
/// CR/LF; address-list entries are trimmed, non-empty, contain @ and contain no
/// comma; optional strings are non-empty; references are non-empty tokens with
/// no whitespace; paths are absolute UTF-8 without CR/LF; the body has no CRLF
/// or trailing whitespace; and the entire encoded document fits the size cap.
/// Single From/Reply-To values need @. This function assumes valid input;
/// parsing validates it. A non-UTF-8 attachment path panics during encoding.
pub fn write(document: &MailDocument) -> String {
    let mut message = BusMessage::new();
    for (name, values) in [
        ("mail-to", &document.to),
        ("mail-cc", &document.cc),
        ("mail-bcc", &document.bcc),
    ] {
        set(&mut message, name, &values.join(", "));
    }
    for (name, value) in [
        ("mail-from", document.from.as_deref()),
        ("mail-reply-to", document.reply_to.as_deref()),
        ("mail-subject", Some(document.subject.as_str())),
        ("mail-in-reply-to", document.in_reply_to.as_deref()),
    ] {
        if let Some(value) = value {
            set(&mut message, name, value);
        }
    }
    set(&mut message, "mail-references", &document.references.join(" "));
    if !document.attachments.is_empty() {
        message.set(
            "mail-attachments",
            &serde_json::to_string(&document.attachments)
                .expect("valid attachment paths are UTF-8"),
        );
    }
    // A blank header line keeps empty documents readable by wire::parse_lenient.
    let mut output = if message.headers.is_empty() {
        "---\n\n---\n".to_owned()
    } else {
        message.to_wire()
    };
    output.push_str(&document.body);
    output
}

#[cfg(test)]
mod tests;
