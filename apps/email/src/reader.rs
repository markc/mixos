// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! How the reader shows the open email's text body: as Markdown or as the
//! plain text it is.
//!
//! The format is detected once, when the email opens: a `text/markdown`
//! body is Markdown; a `text/plain` one is Markdown when
//! [`toolkit::markdown::looks_like_markdown`] says so; anything else
//! (an HTML body served as the text body, no body) is plain. The person
//! can flip it per email (`view.reader.markdown`); flipping back to the
//! detected format returns to automatic. The parsed document is cached
//! here, keyed by the email id and a hash of its body, so it is parsed once
//! per email and never per frame.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use jmap::Email;
use serde::Serialize;
use serde_json::{Value, json};
use toolkit::markdown::{Document, looks_like_markdown};

/// How a text body is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Plain,
    Markdown,
}

impl Format {
    fn other(self) -> Self {
        match self {
            Self::Plain => Self::Markdown,
            Self::Markdown => Self::Plain,
        }
    }
}

/// The open email's text body and its part type, lowercased and without
/// parameters (`text/plain; charset=utf-8` is `text/plain`).
fn text_body(email: &Email) -> Option<(&str, String)> {
    let body = email.text_body_value()?;
    let kind = email
        .text_body
        .as_ref()
        .and_then(|parts| parts.first())
        .and_then(|part| part.content_type.as_deref())
        .map(|t| t.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
        .unwrap_or_default();
    Some((body, kind))
}

/// The format `email`'s text body is shown in unless the person chooses.
pub fn detect(email: &Email) -> Format {
    match text_body(email) {
        Some((_, kind)) if kind == "text/markdown" || kind == "text/x-markdown" => {
            Format::Markdown
        }
        // A server that leaves the type out means plain text.
        Some((body, kind)) if (kind == "text/plain" || kind.is_empty()) && looks_like_markdown(body) => {
            Format::Markdown
        }
        _ => Format::Plain,
    }
}

fn hash(body: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    body.hash(&mut hasher);
    hasher.finish()
}

/// The reader's state for the open email.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reader {
    /// The email id and body hash everything below is for.
    key: Option<(String, u64)>,
    detected: Format,
    /// The person's choice for this email, when it differs from `detected`.
    chosen: Option<Format>,
    /// The parsed body, once it has been shown as Markdown.
    document: Option<Arc<Document>>,
}

impl Reader {
    fn key(email: &Email) -> Option<(String, u64)> {
        text_body(email).map(|(body, _)| (email.id.clone(), hash(body)))
    }

    /// Whether this state is for `email` as it is now.
    pub fn is_for(&self, email: &Email) -> bool {
        self.key.is_some() && self.key == Self::key(email)
    }

    /// Make this the reader for `email`: detect its format and parse it
    /// when it is Markdown. The same email (same body) keeps its choice and
    /// its parse.
    pub fn open(&mut self, email: &Email) {
        if self.is_for(email) {
            return;
        }
        *self = Self {
            key: Self::key(email),
            detected: detect(email),
            ..Self::default()
        };
        self.parse(email);
    }

    fn parse(&mut self, email: &Email) {
        if self.format() == Format::Markdown
            && self.document.is_none()
            && let Some((body, _)) = text_body(email)
        {
            self.document = Some(Arc::new(Document::parse(body)));
        }
    }

    /// The format shown: the person's choice, else the detected one.
    pub fn format(&self) -> Format {
        self.chosen.unwrap_or(self.detected)
    }

    /// Whether the format is the detected one (no choice made).
    pub fn auto(&self) -> bool {
        self.chosen.is_none()
    }

    /// Flip `email` between Markdown and its source.
    pub fn toggle(&mut self, email: &Email) {
        self.open(email);
        if self.key.is_none() {
            return;
        }
        let next = self.format().other();
        self.chosen = (next != self.detected).then_some(next);
        self.parse(email);
    }

    /// The parsed document, when `email` is shown as Markdown.
    pub fn document(&self, email: &Email) -> Option<&Document> {
        (self.is_for(email) && self.format() == Format::Markdown)
            .then(|| self.document.as_deref())
            .flatten()
    }

    /// `email.info`'s `reader` for `email`.
    pub fn info(&self, email: &Email) -> Value {
        if self.is_for(email) {
            json!({ "format": self.format(), "auto": self.auto() })
        } else {
            json!({ "format": detect(email), "auto": true })
        }
    }
}

/// Whether `email` has a text body the reader can show either way.
pub fn has_text(email: &Email) -> bool {
    text_body(email).is_some_and(|(_, kind)| kind != "text/html")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn email(id: &str, kind: Option<&str>, body: &str) -> Email {
        serde_json::from_value(json!({
            "id": id,
            "textBody": [{ "partId": "1", "type": kind }],
            "bodyValues": { "1": { "value": body } },
        }))
        .unwrap()
    }

    const MD: &str = "# Plan\n\n- one\n- two\n";

    #[test]
    fn the_part_type_and_the_detector_choose_the_format() {
        assert_eq!(detect(&email("a", Some("text/markdown"), "hi")), Format::Markdown);
        assert_eq!(
            detect(&email("a", Some("text/markdown; charset=utf-8"), "hi")),
            Format::Markdown
        );
        assert_eq!(detect(&email("a", Some("text/plain"), MD)), Format::Markdown);
        assert_eq!(detect(&email("a", None, MD)), Format::Markdown);
        assert_eq!(detect(&email("a", Some("text/plain"), "Lunch?")), Format::Plain);
        // An HTML part standing in as the text body is never Markdown.
        assert_eq!(detect(&email("a", Some("text/html"), MD)), Format::Plain);
        assert!(!has_text(&email("a", Some("text/html"), MD)));
    }

    #[test]
    fn the_parse_is_cached_per_email_and_body() {
        let a = email("a", Some("text/plain"), MD);
        let mut reader = Reader::default();
        reader.open(&a);
        let first = reader.document.clone().unwrap();
        reader.open(&a);
        assert!(Arc::ptr_eq(&first, reader.document.as_ref().unwrap()), "not reparsed");
        // A changed body is a new key.
        let edited = email("a", Some("text/plain"), "# Plan\n\n## Two\n");
        reader.open(&edited);
        assert!(!Arc::ptr_eq(&first, reader.document.as_ref().unwrap()));
        assert!(reader.document(&a).is_none(), "stale for the old body");
    }

    #[test]
    fn a_toggle_flips_and_flipping_back_is_auto() {
        let a = email("a", Some("text/plain"), MD);
        let mut reader = Reader::default();
        reader.open(&a);
        assert_eq!((reader.format(), reader.auto()), (Format::Markdown, true));
        reader.toggle(&a);
        assert_eq!((reader.format(), reader.auto()), (Format::Plain, false));
        assert!(reader.document(&a).is_none());
        reader.toggle(&a);
        assert_eq!((reader.format(), reader.auto()), (Format::Markdown, true));
        assert_eq!(reader.info(&a), json!({"format": "markdown", "auto": true}));
        // A plain email can be shown as Markdown, parsed on demand.
        let b = email("b", Some("text/plain"), "Lunch?");
        reader.toggle(&b);
        assert_eq!((reader.format(), reader.auto()), (Format::Markdown, false));
        assert!(reader.document(&b).is_some());
        // Another email starts automatic again.
        reader.open(&a);
        assert!(reader.auto());
    }
}
