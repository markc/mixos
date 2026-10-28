// SPDX-License-Identifier: MIT OR Apache-2.0
//! Inert document classification and editable mail headers. No mail transport.
use maildoc::MailDocument;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    let Some(rest) = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) else {
        return Kind::Text;
    };
    let lines: Vec<_> = rest.lines().take_while(|line| *line != "---").collect();
    let names: Vec<_> = lines.iter().copied()
        .filter_map(|line| line.split_once(':').map(|(name, _)| name.trim())).collect();
    let mail = lines.iter().any(|line| line.trim_start().starts_with("mail-"));
    let empty = lines.iter().all(|line| line.is_empty()) && rest.lines().any(|line| line == "---");
    if mail || empty {
        match maildoc::parse(text) {
            Ok(doc) => Kind::Mail(doc),
            Err(error) => Kind::Refused(error.to_string()),
        }
    } else if names.contains(&"to") && names.contains(&"command") {
        Kind::BusMessage
    } else {
        Kind::Text
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
            from: doc.from.clone().unwrap_or_default(), to: doc.to.join(", "),
            cc: doc.cc.join(", "), bcc: doc.bcc.join(", "),
            reply_to: doc.reply_to.clone().unwrap_or_default(), subject: doc.subject.clone(),
            in_reply_to: doc.in_reply_to.clone(), references: doc.references.clone(),
            attachments: doc.attachments.clone(),
        }
    }

    pub fn document(&self, body: String) -> MailDocument {
        let addresses = |value: &str| if value.is_empty() { Vec::new() } else {
            value.split(',').map(|s| s.trim().to_owned()).collect()
        };
        let optional = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        MailDocument {
            from: optional(&self.from), to: addresses(&self.to), cc: addresses(&self.cc),
            bcc: addresses(&self.bcc), reply_to: optional(&self.reply_to), subject: self.subject.clone(),
            in_reply_to: self.in_reply_to.clone(), references: self.references.clone(),
            attachments: self.attachments.clone(), body,
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
    pub(crate) disk: String,
    pub(crate) undo: Vec<Headers>,
    pub(crate) redo: Vec<Headers>,
    pub(crate) seeding: bool,
}

impl MailState {
    pub fn new(document: MailDocument, disk: String) -> Self {
        Self { headers: Headers::of(&document), sending: false, saving: false, notice: None,
            show_cc: !document.cc.is_empty(), show_bcc: !document.bcc.is_empty(),
            show_reply_to: document.reply_to.is_some(), saved: document, disk,
            undo: Vec::new(), redo: Vec::new(), seeding: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifier_observes_fences_and_header_names() {
        assert!(matches!(classify("hello\nmail-to: a@example.org"), Kind::Text));
        assert!(matches!(classify("---\nThis is a rule above prose, not front matter."), Kind::Text));
        assert!(matches!(classify("---\n\n---\nBody"), Kind::Mail(_)));
        assert!(matches!(classify("---\ntitle: Notes\n---\ntext"), Kind::Text));
        assert!(matches!(classify("---\nto: edit\ncommand: edit.info\n---\n"), Kind::BusMessage));
        assert!(matches!(classify("---\nmail-to: a@example.org\n---\nBody"), Kind::Mail(_)));
        for text in ["---\nmail-to: bad\n---\nBody", "---\nmail-to: a@example.org\nmail-to: b@example.org\n---\nBody", "---\r\nmail-to: a@example.org\r\n---\r\nBody", "---\nmail-to: a@example.org\ncommand: go\n---\nBody"] {
            assert_eq!(classify(text), Kind::Refused(maildoc::parse(text).unwrap_err().to_string()));
        }
    }
}
