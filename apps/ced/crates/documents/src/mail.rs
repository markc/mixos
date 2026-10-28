// SPDX-License-Identifier: MIT OR Apache-2.0
//! Inert document classification and editable mail headers. No mail transport.
use maildoc::MailDocument;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const INTERNAL: &str = "INTERNAL";

/// Save refusals retain their category independently of their wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    Argument(String),
    Conflict(String),
    Internal(String),
    Document(maildoc::Refusal),
}

impl SaveError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Argument(_) => "INVALID_ARGUMENT",
            Self::Conflict(_) => "CONFLICT",
            Self::Internal(_) => INTERNAL,
            Self::Document(refusal) => refusal_code(refusal),
        }
    }
}

pub fn refusal_code(refusal: &maildoc::Refusal) -> &'static str {
    match refusal {
        maildoc::Refusal::TooLarge { .. }
        | maildoc::Refusal::CarriageReturn
        | maildoc::Refusal::Framing(_)
        | maildoc::Refusal::HeaderNotAllowed(_)
        | maildoc::Refusal::MalformedHeader(_)
        | maildoc::Refusal::DuplicateHeader(_)
        | maildoc::Refusal::EmptyValue(_)
        | maildoc::Refusal::InvalidValue { .. } => "INVALID_ARGUMENT",
    }
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Argument(message) | Self::Conflict(message) | Self::Internal(message) => f.write_str(message),
            Self::Document(error) => error.fmt(f),
        }
    }
}

impl From<std::io::Error> for SaveError {
    fn from(error: std::io::Error) -> Self { Self::Internal(error.to_string()) }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

impl Failure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    BusMessage,
    Mail(MailDocument),
    Refused(String),
}

/// Look only inside an opening front-matter block. A mail header makes a
/// mixed block a mail candidate too, so maildoc can explain its refusal.
pub fn classify(text: &str) -> Kind {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return Kind::Text;
    };
    let lines: Vec<_> = rest.lines().take_while(|line| *line != "---").collect();
    let names: Vec<_> = lines
        .iter()
        .copied()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| line.split_once(':').map(|(name, _)| name.trim()))
        .collect();
    let mail = names.iter().any(|name| name.starts_with("mail-"));
    let closed = rest.lines().any(|line| line == "---");
    if mail {
        match maildoc::parse(text) {
            Ok(doc) => Kind::Mail(doc),
            Err(error) => Kind::Refused(error.to_string()),
        }
    } else if closed && names.contains(&"to") && names.contains(&"command") {
        Kind::BusMessage
    } else {
        Kind::Text
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Headers {
    pub from: String,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub reply_to: String,
    pub subject: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub attachments: Vec<PathBuf>,
}

impl Headers {
    pub fn of(doc: &MailDocument) -> Self {
        Self {
            from: doc.from.clone().unwrap_or_default(),
            to: doc.to.join(", "),
            cc: doc.cc.join(", "),
            bcc: doc.bcc.join(", "),
            reply_to: doc.reply_to.clone().unwrap_or_default(),
            subject: doc.subject.clone(),
            in_reply_to: doc.in_reply_to.clone(),
            references: doc.references.clone(),
            attachments: doc.attachments.clone(),
        }
    }

    pub fn document(&self, body: String) -> MailDocument {
        let addresses = |value: &str| {
            if value.is_empty() {
                Vec::new()
            } else {
                value.split(',').map(|s| s.trim().to_owned()).collect()
            }
        };
        let optional = |value: &str| (!value.trim().is_empty()).then(|| value.trim().to_owned());
        MailDocument {
            from: optional(&self.from),
            to: addresses(&self.to),
            cc: addresses(&self.cc),
            bcc: addresses(&self.bcc),
            reply_to: optional(&self.reply_to),
            subject: self.subject.clone(),
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            attachments: self.attachments.clone(),
            body,
        }
    }
}

pub struct MailState {
    pub headers: Headers,
    pub sending: bool,
    pub saving: bool,
    pub notice: Option<String>,
    pub show_cc: bool,
    pub show_bcc: bool,
    pub show_reply_to: bool,
    pub(crate) saved: MailDocument,
    pub(crate) saved_headers: Headers,
    pub(crate) saved_body: String,
    pub(crate) saving_body: Option<String>,
    pub(crate) dirty_body: std::cell::Cell<Option<(u64, bool)>>,
    pub(crate) disk: String,
    pub(crate) undo: Vec<Headers>,
    pub(crate) redo: Vec<Headers>,
    pub(crate) seeding: bool,
    pub(crate) editing_field: Option<&'static str>,
}

impl MailState {
    pub fn new(document: MailDocument, disk: String) -> Self {
        Self {
            headers: Headers::of(&document),
            saved_headers: Headers::of(&document),
            saved_body: document.body.clone(),
            saving_body: None,
            dirty_body: std::cell::Cell::new(None),
            sending: false,
            saving: false,
            notice: None,
            show_cc: !document.cc.is_empty(),
            show_bcc: !document.bcc.is_empty(),
            show_reply_to: document.reply_to.is_some(),
            saved: document,
            disk,
            undo: Vec::new(),
            redo: Vec::new(),
            seeding: true,
            editing_field: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_headers_default_and_sender_addresses_are_trimmed() {
        let mut headers: Headers = serde_json::from_str(r#"{"to":"bob@example.org"}"#).unwrap();
        assert!(headers.cc.is_empty() && headers.attachments.is_empty());
        headers.from = "  ann@example.org  ".into();
        headers.reply_to = " reply@example.org ".into();
        let document = headers.document("Body".into());
        assert_eq!(document.from.as_deref(), Some("ann@example.org"));
        assert_eq!(document.reply_to.as_deref(), Some("reply@example.org"));
        assert!(maildoc::write(&document).is_ok());
    }

    #[test]
    fn classifier_observes_fences_and_header_names() {
        assert_eq!(classify("---\nto: edit\ncommand: edit.info"), Kind::Text);
        assert!(matches!(
            classify("hello\nmail-to: a@example.org"),
            Kind::Text
        ));
        assert!(matches!(
            classify("---\nThis is a rule above prose, not front matter."),
            Kind::Text
        ));
        assert_eq!(classify("---\n\n---\nBody"), Kind::Text);
        assert_eq!(
            classify("---\ntitle: Notes\n mail-to: bob@example.org\n---\nBody"),
            Kind::Text
        );
        assert_eq!(classify("---\nmail-without-a-colon\n---\nBody"), Kind::Text);
        assert!(matches!(
            classify("---\ntitle: Notes\n---\ntext"),
            Kind::Text
        ));
        assert!(matches!(
            classify("---\nto: edit\ncommand: edit.info\n---\n"),
            Kind::BusMessage
        ));
        assert!(matches!(
            classify("---\nmail-to: a@example.org\n---\nBody"),
            Kind::Mail(_)
        ));
        for text in [
            "---\nmail-to: bad\n---\nBody",
            "---\nmail-to: a@example.org\nmail-to: b@example.org\n---\nBody",
            "---\r\nmail-to: a@example.org\r\n---\r\nBody",
            "---\nmail-to: a@example.org\ncommand: go\n---\nBody",
        ] {
            assert_eq!(
                classify(text),
                Kind::Refused(maildoc::parse(text).unwrap_err().to_string())
            );
        }
    }
}
