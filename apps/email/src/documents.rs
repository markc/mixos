// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Mail document effects and fenced state transitions. Opening never sends.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use maildoc::{MailDocument, Refusal};

use crate::state::{State, code};
use crate::work::Work;

pub type Failure = (&'static str, String);
pub type Result<T> = std::result::Result<T, Failure>;

pub fn refusal(error: Refusal) -> Failure {
    let code = match &error {
        Refusal::TooLarge { .. } => code::DOCUMENT_TOO_LARGE,
        Refusal::CarriageReturn => code::DOCUMENT_CARRIAGE_RETURN,
        Refusal::Framing(_) => code::DOCUMENT_FRAMING,
        Refusal::HeaderNotAllowed(_) => code::DOCUMENT_HEADER_NOT_ALLOWED,
        Refusal::MalformedHeader(_) => code::DOCUMENT_MALFORMED_HEADER,
        Refusal::DuplicateHeader(_) => code::DOCUMENT_DUPLICATE_HEADER,
        Refusal::EmptyValue(_) => code::DOCUMENT_EMPTY_VALUE,
        Refusal::InvalidValue { .. } => code::DOCUMENT_INVALID_VALUE,
    };
    (code, error.to_string())
}

/// Runs only on the blocking pool. Nonblocking open avoids hanging on a FIFO;
/// take bounds allocations even if a regular file grows after metadata().
pub fn read(path: &std::path::Path) -> Result<MailDocument> {
    let io = |error: std::io::Error| (code::DOCUMENT_IO, error.to_string());
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(io)?;
    if !file.metadata().map_err(io)?.is_file() {
        return Err((code::DOCUMENT_IO, crate::label("document-not-file")));
    }
    let mut text = Vec::new();
    file.take(maildoc::MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut text)
        .map_err(io)?;
    if text.len() > maildoc::MAX_DOCUMENT_BYTES {
        return Err(refusal(Refusal::TooLarge { bytes: text.len() }));
    }
    let text =
        String::from_utf8(text).map_err(|error| (code::DOCUMENT_ENCODING, error.to_string()))?;
    maildoc::parse(&text).map_err(refusal)
}

/// Keep an approved replacement intact until its sibling is fully written.
fn save_file(
    path: &std::path::Path,
    replace: bool,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    if !replace {
        let mut file = options.open(path)?;
        return write(&mut file).and_then(|()| file.sync_all());
    }
    // Refuse links and special files just as the non-replacement path does.
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(std::io::Error::other(crate::label("document-not-file")));
    }
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let (temporary, mut file) = loop {
        let temporary = path.with_file_name(format!(
            ".email-save-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        match options.open(&temporary) {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = write(&mut file)
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[derive(Clone, Debug, PartialEq)]
pub enum DocumentWork {
    Folder {
        ticket: u64,
    },
    Read {
        ticket: u64,
        path: PathBuf,
    },
    Save {
        ticket: u64,
        path: PathBuf,
        replace: bool,
        document: MailDocument,
    },
}

#[derive(Debug)]
pub enum Answer {
    Folder(PathBuf),
    Read(Result<MailDocument>),
    Saved(Result<()>),
}

impl DocumentWork {
    pub fn perform(self) -> (u64, Answer) {
        match self {
            Self::Folder { ticket } => (ticket, Answer::Folder(toolkit::file_chooser::documents())),
            Self::Read { ticket, path } => (ticket, Answer::Read(read(&path))),
            Self::Save {
                ticket,
                path,
                replace,
                document,
            } => {
                let result = maildoc::write(&document).map_err(refusal).and_then(|text| {
                    // A destination which appeared after the chooser checked it
                    // must never be overwritten without the replacement question.
                    save_file(&path, replace, |file| file.write_all(text.as_bytes()))
                        .map_err(|e| (code::DOCUMENT_IO, e.to_string()))
                });
                (ticket, Answer::Saved(result))
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Request {
    account: String,
    generation: u64,
    fence: Option<(String, u64)>,
    reply: Option<u64>,
    send: bool,
    dropped: bool,
    path: PathBuf,
}

#[derive(Default)]
pub struct DocumentState {
    pending: BTreeMap<u64, Request>,
    folder: Option<(u64, bool, Option<(String, u64)>)>,
    saving: BTreeMap<u64, Option<(String, u64)>>,
    pub chooser: Option<toolkit::file_chooser::FileChooser>,
    pub last_directory: Option<PathBuf>,
    pub replace: Option<(u64, MailDocument)>,
}

impl DocumentState {
    pub fn opening(&self) -> bool {
        self.pending.values().any(|r| !r.send)
    }
    pub fn modal(&self) -> bool {
        self.chooser.is_some() || self.replace.is_some() || self.folder.is_some()
    }
}

impl State {
    pub(crate) fn reset_document_modals(&mut self) {
        self.documents.chooser = None;
        self.documents.replace = None;
        self.documents.folder = None;
    }

    pub(crate) fn cancel_document_open(&mut self) {
        self.reset_document_modals();
        let tickets: Vec<_> = self
            .documents
            .pending
            .iter()
            .filter_map(|(&ticket, request)| (!request.send).then_some(ticket))
            .collect();
        for ticket in tickets {
            self.finish_document_error(
                ticket,
                (code::CANCELLED, crate::label("closing-cancelled")),
            );
        }
    }

    fn document_fence(&self) -> Option<(String, u64)> {
        self.compose
            .as_ref()
            .map(|c| (c.draft_id.clone(), c.revision))
    }

    fn document_error(&mut self, error: Failure) {
        if let Some(compose) = &mut self.compose {
            compose.error = Some(error.1);
        } else {
            self.status = error.1;
        }
    }

    pub fn read_document(
        &mut self,
        account: String,
        path: PathBuf,
        reply: Option<u64>,
        send: bool,
        dropped: bool,
    ) -> Result<()> {
        if !path.is_absolute() {
            return Err((code::DOCUMENT_PATH, crate::label("document-absolute-path")));
        }
        if self.documents.pending.len() >= 8
            || (!send
                && (self.documents.opening() || self.compose.as_ref().is_some_and(|c| c.busy)))
        {
            return Err((code::BUSY, crate::label("compose-open")));
        }
        let generation = self
            .account(&account)
            .ok_or_else(|| (code::UNKNOWN_ACCOUNT, crate::label("no-account")))?
            .generation();
        let ticket = self.ticket();
        self.documents.pending.insert(
            ticket,
            Request {
                account,
                generation,
                fence: self.document_fence(),
                reply,
                send,
                dropped,
                path: path.clone(),
            },
        );
        self.queue_work(Work::MailDocument(DocumentWork::Read { ticket, path }));
        if let Some(id) = reply {
            self.await_document(id);
        }
        Ok(())
    }

    pub fn open_document(&mut self) {
        self.choose_document(false);
    }
    pub fn save_document_as(&mut self) {
        self.choose_document(true);
    }

    fn choose_document(&mut self, save: bool) {
        if self
            .compose
            .as_ref()
            .is_none_or(|c| c.busy || !c.attachment_inspections.is_empty())
            || self.documents.modal()
            || self.documents.opening()
        {
            return;
        }
        if let Some(directory) = self.documents.last_directory.clone() {
            self.document_chooser(directory, save);
        } else {
            let ticket = self.ticket();
            self.documents.folder = Some((ticket, save, self.document_fence()));
            self.queue_work(Work::MailDocument(DocumentWork::Folder { ticket }));
        }
    }

    fn document_chooser(&mut self, directory: PathBuf, save: bool) {
        self.documents.chooser = Some(if save {
            toolkit::file_chooser::FileChooser::save(
                directory,
                crate::label("document-save-title"),
                crate::label("document-save"),
            )
        } else {
            toolkit::file_chooser::FileChooser::single(
                directory,
                crate::label("document-open-title"),
                crate::label("document-open"),
            )
        });
    }

    /// Called by the shell after drawing; I/O remains queued as effects.
    pub fn document_chooser_ui(&mut self, ctx: &egui::Context) {
        if self.compose.is_none() {
            self.reset_document_modals();
        }
        let outcome = self.documents.chooser.as_mut().and_then(|c| c.show(ctx));
        if let Some(outcome) = outcome {
            self.documents.last_directory = self
                .documents
                .chooser
                .as_ref()
                .map(|c| c.directory().to_owned());
            self.documents.chooser = None;
            match outcome {
                toolkit::file_chooser::Outcome::Picked(paths) => {
                    if let Some(path) = paths.into_iter().next() {
                        let account = self
                            .compose
                            .as_ref()
                            .map(|c| c.account.clone())
                            .unwrap_or_default();
                        if let Err(error) = self.read_document(account, path, None, false, false) {
                            self.document_error(error);
                        }
                    }
                }
                toolkit::file_chooser::Outcome::Save { path, replace } => {
                    self.write_document(path, replace)
                }
                toolkit::file_chooser::Outcome::Cancelled => {}
            }
            ctx.request_repaint();
        }
    }

    pub fn write_document(&mut self, path: PathBuf, replace: bool) {
        let Some(compose) = self
            .compose
            .as_ref()
            .filter(|c| !c.busy && c.attachment_inspections.is_empty())
        else {
            return;
        };
        let addresses = |text: &str| {
            jmap::parse_addresses(text)
                .map(|a| a.iter().map(jmap::format_address).collect::<Vec<_>>())
                .map_err(|e| (code::DOCUMENT_INVALID_VALUE, e.to_string()))
        };
        let document = (|| {
            Ok(MailDocument {
                to: addresses(&compose.to_text)?,
                cc: addresses(&compose.cc_text)?,
                bcc: addresses(&compose.bcc_text)?,
                from: Some(compose.draft.from.clone()),
                reply_to: compose.draft.reply_to.clone(),
                subject: compose.draft.subject.clone(),
                body: compose.body.contents().trim_end().to_owned(),
                in_reply_to: compose.draft.in_reply_to.clone(),
                references: compose.draft.references.clone(),
                attachments: compose.draft.attachments.clone(),
            })
        })();
        match document {
            Ok(document) => {
                let ticket = self.ticket();
                self.documents.saving.insert(ticket, self.document_fence());
                self.queue_work(Work::MailDocument(DocumentWork::Save {
                    ticket,
                    path,
                    replace,
                    document,
                }));
            }
            Err(error) => self.document_error(error),
        }
    }

    pub fn drop_files(&mut self, paths: Vec<PathBuf>) {
        let mut attachments = Vec::new();
        for path in paths {
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                let account = self
                    .compose
                    .as_ref()
                    .map(|c| c.account.clone())
                    .or_else(|| self.compose_account());
                let result = account
                    .ok_or_else(|| (code::UNKNOWN_ACCOUNT, crate::label("no-account")))
                    .and_then(|account| self.read_document(account, path, None, false, true));
                if let Err(error) = result {
                    self.document_error(error);
                }
            } else {
                attachments.push(path);
            }
        }
        if !attachments.is_empty()
            && self.compose.is_some()
            && let Err(error) = self.add_compose_attachments(&attachments)
        {
            self.document_error(error);
        }
    }

    pub fn document_done(&mut self, ticket: u64, answer: Answer) {
        match answer {
            Answer::Folder(directory) => {
                if let Some((expected, save, fence)) = self.documents.folder.take()
                    && expected == ticket
                    && fence == self.document_fence()
                {
                    self.document_chooser(directory, save);
                }
            }
            Answer::Saved(result) => {
                let current = self
                    .documents
                    .saving
                    .remove(&ticket)
                    .is_some_and(|fence| fence == self.document_fence());
                match result {
                    Err(error) => self.document_error(error),
                    Ok(()) => {
                        if current && let Some(compose) = &mut self.compose {
                            compose.error = None;
                        }
                    }
                }
            }
            Answer::Read(result) => {
                let Some(request) = self.documents.pending.get(&ticket).cloned() else {
                    return;
                };
                let result = if self
                    .account(&request.account)
                    .is_none_or(|v| v.generation() != request.generation)
                {
                    Err((code::STALE, crate::label("not-connected")))
                } else if !request.send
                    && (request.fence != self.document_fence()
                        || self.compose.as_ref().is_some_and(|c| c.busy))
                {
                    Err((code::SUPERSEDED, crate::label("document-superseded")))
                } else {
                    result
                };
                match result {
                    Ok(document) => {
                        // Check identity before offering a replacement.
                        if let Err(error) = composed(self, &request.account, &document) {
                            self.finish_document_error(ticket, error);
                        } else if !request.send
                            && request.reply.is_none()
                            && self.compose.as_ref().is_some_and(|c| c.keepable())
                        {
                            self.documents.replace = Some((ticket, document));
                        } else {
                            self.install_document(ticket, document);
                        }
                    }
                    Err(error) => {
                        if request.dropped
                            && request.fence.is_some()
                            && request.fence == self.document_fence()
                            && !matches!(error.0, code::STALE | code::SUPERSEDED)
                        {
                            self.documents.pending.remove(&ticket);
                            if let Err(error) = self.add_compose_attachments(&[request.path]) {
                                self.document_error(error);
                            }
                        } else {
                            self.finish_document_error(ticket, error);
                        }
                    }
                }
            }
        }
    }

    fn finish_document_error(&mut self, ticket: u64, error: Failure) {
        if let Some(request) = self.documents.pending.remove(&ticket) {
            if let Some(id) = request.reply {
                self.refuse(id, error.0, &error.1);
            } else {
                self.document_error(error);
            }
        }
    }

    pub fn confirm_document_replace(&mut self, replace: bool) {
        if let Some((ticket, document)) = self.documents.replace.take() {
            if replace {
                self.install_document(ticket, document);
            } else {
                self.documents.pending.remove(&ticket);
            }
        }
    }

    fn install_document(&mut self, ticket: u64, document: MailDocument) {
        let Some(request) = self.documents.pending.remove(&ticket) else {
            return;
        };
        let result = (|| {
            if self
                .account(&request.account)
                .is_none_or(|v| v.generation() != request.generation)
            {
                return Err((code::STALE, crate::label("not-connected")));
            }
            if !request.send && self.compose.as_ref().is_some_and(|c| c.busy) {
                return Err((code::BUSY, crate::label("compose-open")));
            }
            if !request.send && request.fence != self.document_fence() {
                return Err((code::SUPERSEDED, crate::label("document-superseded")));
            }
            let (mut draft, identity) = composed(self, &request.account, &document)?;
            if request.send {
                if self
                    .account(&request.account)
                    .is_none_or(|v| v.phase != crate::state::Phase::Ready)
                {
                    return Err((code::NOT_CONNECTED, crate::label("not-connected")));
                }
                if draft.to.is_empty() || draft.subject.trim().is_empty() {
                    return Err((
                        code::INVALID_ARGUMENT,
                        crate::label("document-send-required"),
                    ));
                }
                self.write_server(
                    request.account.clone(),
                    request.reply,
                    crate::commands::Write::Send { draft },
                );
            } else {
                let paths = std::mem::take(&mut draft.attachments);
                if let Some(compose) = self.take_compose() {
                    self.discard_draft(&compose.draft_id);
                }
                self.open_compose(Some(&request.account), draft.clone());
                let compose = self
                    .compose
                    .as_mut()
                    .ok_or_else(|| (code::UNKNOWN_ACCOUNT, crate::label("no-account")))?;
                compose.draft.from = draft.from;
                compose.draft.from_name = draft.from_name;
                compose.draft.reply_to = draft.reply_to;
                compose.identity = identity;
                compose.set_body(&document.body);
                if let Some(error) = self.close_failed_compose() {
                    return Err((code::DOCUMENT_INVALID_VALUE, error));
                }
                self.inspect_compose_attachments(&paths, request.reply, true)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            if let Some(id) = request.reply {
                self.refuse(id, error.0, &error.1);
            } else {
                self.document_error(error);
            }
        }
    }
}

/// Select an account-owned From address. Inline mail may use any Reply-To.
pub fn identity(
    state: &State,
    account: &str,
    from: Option<&str>,
) -> Result<settings::email::Identity> {
    let senders = state.senders(account);
    let mailbox = |value: &str| -> Result<String> {
        let addresses =
            jmap::parse_addresses(value).map_err(|e| (code::IDENTITY_MISMATCH, e.to_string()))?;
        if addresses.len() != 1 {
            return Err((code::IDENTITY_MISMATCH, crate::label("document-identity")));
        }
        Ok(addresses[0].email.clone())
    };
    let from = from.map(mailbox).transpose()?;
    let sender = senders
        .iter()
        .find(|s| {
            from.as_ref()
                .is_none_or(|f| s.address.eq_ignore_ascii_case(f))
        })
        .cloned()
        .ok_or_else(|| (code::IDENTITY_MISMATCH, crate::label("document-identity")))?;
    Ok(sender)
}

fn composed(state: &State, account: &str, doc: &MailDocument) -> Result<(jmap::Composed, String)> {
    let sender = identity(state, account, doc.from.as_deref())?;
    // Decision 1 restricts document Reply-To to an owned identity too.
    if let Some(reply_to) = doc.reply_to.as_deref() {
        identity(state, account, Some(reply_to))?;
    }
    let addresses = |values: &[String]| {
        jmap::parse_addresses(&values.join(", "))
            .map_err(|e| (code::DOCUMENT_INVALID_VALUE, e.to_string()))
    };
    Ok((
        jmap::Composed {
            from: sender.address,
            from_name: sender.name,
            reply_to: doc.reply_to.clone(),
            to: addresses(&doc.to)?,
            cc: addresses(&doc.cc)?,
            bcc: addresses(&doc.bcc)?,
            subject: doc.subject.clone(),
            body: doc.body.clone(),
            attachments: doc.attachments.clone(),
            in_reply_to: doc.in_reply_to.clone(),
            references: doc.references.clone(),
        },
        sender.id,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Event;
    use crate::state::tests::connected_many;
    use serde_json::{Value, json};

    fn command(state: &mut State, name: &str, args: Value) {
        crate::commands::command(
            state,
            &crate::actions::registry(),
            &crate::strings(),
            71,
            name,
            &args.to_string(),
        );
    }

    fn finish(state: &mut State) {
        for work in state.take_work() {
            match work {
                Work::MailDocument(work) => {
                    let (ticket, answer) = work.perform();
                    state.apply(Event::MailDocument(ticket, answer));
                }
                other => state.queue_work(other),
            }
        }
        state.finish_compose_inspections();
    }

    #[test]
    fn bus_documents_have_distinct_format_refusals_on_compose_and_send() {
        let cases = [
            ("x".into(), code::DOCUMENT_FRAMING),
            ("---\r\n---\r\n".into(), code::DOCUMENT_CARRIAGE_RETURN),
            (
                "---\nto: email\n---\n".into(),
                code::DOCUMENT_HEADER_NOT_ALLOWED,
            ),
            (
                "---\nmail-to\n---\n".into(),
                code::DOCUMENT_MALFORMED_HEADER,
            ),
            (
                "---\nmail-subject: A\nmail-subject: B\n---\n".into(),
                code::DOCUMENT_DUPLICATE_HEADER,
            ),
            (
                "---\nmail-subject: \"\"\n---\n".into(),
                code::DOCUMENT_EMPTY_VALUE,
            ),
            (
                "---\nmail-attachments: [\"relative\"]\n---\n".into(),
                code::DOCUMENT_INVALID_VALUE,
            ),
            (
                "x".repeat(maildoc::MAX_DOCUMENT_BYTES + 1),
                code::DOCUMENT_TOO_LARGE,
            ),
        ];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.md");
        for name in ["email.compose", "email.send"] {
            for (text, expected) in &cases {
                let mut state = connected_many(&["a"]);
                std::fs::write(&path, text).unwrap();
                command(&mut state, name, json!({"document": path}));
                assert!(state.take_replies().is_empty(), "read must be deferred");
                finish(&mut state);
                assert_eq!(state.take_replies()[0].body["error_code"], *expected);
                assert!(state.compose.is_none());
            }
        }
    }

    #[test]
    fn bus_documents_refuse_inline_fields_relative_paths_encoding_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.md");
        for name in ["email.compose", "email.send"] {
            for field in [
                "to",
                "cc",
                "bcc",
                "from",
                "from_name",
                "reply_to",
                "subject",
                "body",
                "attachments",
                "in_reply_to",
                "references",
            ] {
                let mut state = connected_many(&["a"]);
                command(&mut state, name, json!({"document": path, (field): null}));
                assert_eq!(
                    state.take_replies()[0].body["error_code"],
                    code::DOCUMENT_INLINE
                );
            }
            let mut state = connected_many(&["a"]);
            command(&mut state, name, json!({"document": "relative.md"}));
            assert_eq!(
                state.take_replies()[0].body["error_code"],
                code::DOCUMENT_PATH
            );
            command(&mut state, name, json!({"document": path}));
            finish(&mut state);
            assert_eq!(
                state.take_replies()[0].body["error_code"],
                code::DOCUMENT_IO
            );
            std::fs::write(&path, [0xff]).unwrap();
            command(&mut state, name, json!({"document": path}));
            finish(&mut state);
            assert_eq!(
                state.take_replies()[0].body["error_code"],
                code::DOCUMENT_ENCODING
            );
            std::fs::remove_file(&path).unwrap();
        }
    }

    #[test]
    fn document_identity_is_scoped_to_the_chosen_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.md");
        for name in ["email.compose", "email.send"] {
            for header in ["mail-from", "mail-reply-to"] {
                for address in ["forged@example.invalid", "ann@b.example.invalid"] {
                    let mut state = connected_many(&["a", "b"]);
                    std::fs::write(&path, format!("---\n{header}: {address}\n---\n")).unwrap();
                    command(&mut state, name, json!({"account": "a", "document": path}));
                    finish(&mut state);
                    assert_eq!(
                        state.take_replies()[0].body["error_code"],
                        code::IDENTITY_MISMATCH
                    );
                }
            }
        }
    }

    #[test]
    fn owned_alias_selects_its_identity_and_missing_from_uses_the_default() {
        let mut state = connected_many(&["a"]);
        let account = state
            .accounts
            .iter_mut()
            .find(|v| v.entry.id == "a")
            .unwrap();
        account.entry.identities = ["default", "sales"]
            .into_iter()
            .map(|id| settings::email::Identity {
                id: id.into(),
                name: id.into(),
                address: format!("{id}@example.invalid"),
                signature: String::new(),
                reply_to: None,
            })
            .collect();
        let doc = MailDocument {
            from: Some("Sales <SALES@example.invalid>".into()),
            reply_to: Some("default@example.invalid".into()),
            ..Default::default()
        };
        let (draft, id) = composed(&state, "a", &doc).unwrap();
        assert_eq!(id, "sales");
        assert_eq!(draft.from, "sales@example.invalid");
        let (_, id) = composed(&state, "a", &MailDocument::default()).unwrap();
        assert_eq!(id, "default");
        command(
            &mut state,
            "email.send",
            json!({"to": "bob@example.invalid", "subject": "Inline",
            "from": "sales@example.invalid", "reply_to": "default@example.invalid", "references": ["<first>", "<second>"]}),
        );
        let draft = state
            .take_work()
            .into_iter()
            .find_map(|w| match w {
                Work::Write {
                    write: crate::commands::Write::Send { draft },
                    ..
                } => Some(draft),
                _ => None,
            })
            .unwrap();
        let mime = String::from_utf8(jmap::build_mime(&draft).unwrap()).unwrap();
        assert!(mime.contains("Reply-To: default@example.invalid\r\n"));
        assert!(mime.contains("References: <first> <second>\r\n"));
        command(
            &mut state,
            "email.send",
            json!({"to": "bob@example.invalid", "subject": "Inline Reply-To", "reply_to": "foreign@example.invalid"}),
        );
        assert!(state.take_replies().is_empty());
        assert!(state.take_work().iter().any(|w| matches!(w,
            Work::Write { write: crate::commands::Write::Send { draft }, .. }
            if draft.reply_to.as_deref() == Some("foreign@example.invalid"))));
    }

    #[test]
    fn composer_sends_with_an_identitys_unowned_reply_to() {
        let mut state = connected_many(&["a"]);
        state.accounts[0].entry.identities = vec![settings::email::Identity {
            id: "ann".into(),
            name: "Ann".into(),
            address: "ann@a.example.invalid".into(),
            signature: String::new(),
            reply_to: Some("help@example.invalid".into()),
        }];
        assert!(state.open_compose(
            Some("a"),
            jmap::Composed {
                to: jmap::parse_addresses("bob@example.invalid").unwrap(),
                subject: "Composer Reply-To".into(),
                ..Default::default()
            }
        ));
        state.send_compose();
        let draft = state
            .take_work()
            .into_iter()
            .find_map(|w| match w {
                Work::Send { draft, .. } => Some(draft),
                _ => None,
            })
            .expect("composer must send");
        assert_eq!(draft.reply_to.as_deref(), Some("help@example.invalid"));
        assert!(
            String::from_utf8(jmap::build_mime(&draft).unwrap())
                .unwrap()
                .contains("Reply-To: help@example.invalid\r\n")
        );
    }

    #[test]
    fn closing_cancels_a_pending_document_request_and_discards_late_reads() {
        let mut state = connected_many(&["a"]);
        command(
            &mut state,
            "email.compose",
            json!({"document": "/tmp/mail.md"}),
        );
        let ticket = *state.documents.pending.keys().next().unwrap();
        state.close_out();
        assert_eq!(state.take_replies()[0].body["error_code"], code::CANCELLED);
        state.document_done(ticket, Answer::Read(Ok(MailDocument::default())));
        assert!(state.compose.is_none());
        assert!(state.take_replies().is_empty());
    }

    #[test]
    fn compose_reads_on_arrival_and_keeps_paths_threading_and_reply_to() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.md");
        let attachment = dir.path().join("note.txt");
        std::fs::write(&attachment, "one").unwrap();
        let mut doc = MailDocument {
            to: vec!["bob@example.invalid".into()],
            subject: "[PATCH] Example".into(),
            reply_to: Some("ann@a.example.invalid".into()),
            body: "**Plain** Markdown".into(),
            references: vec![
                "<first@example.invalid>".into(),
                "<second@example.invalid>".into(),
            ],
            in_reply_to: Some("<second@example.invalid>".into()),
            attachments: vec![attachment.clone()],
            ..MailDocument::default()
        };
        std::fs::write(&path, maildoc::write(&doc).unwrap()).unwrap();
        let mut state = connected_many(&["a"]);
        command(&mut state, "email.compose", json!({"document": path}));
        finish(&mut state);
        assert_eq!(state.take_replies()[0].rc, 0);
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(compose.draft.from, "ann@a.example.invalid");
        assert_eq!(compose.draft.references, doc.references);
        assert_eq!(compose.draft.reply_to, doc.reply_to);
        assert_eq!(compose.body.contents(), doc.body);
        assert_eq!(
            compose.draft.attachments.as_slice(),
            std::slice::from_ref(&attachment)
        );
        std::fs::write(&attachment, "changed at send time").unwrap();
        doc.subject = "Changed document".into();
        std::fs::write(&path, maildoc::write(&doc).unwrap()).unwrap();
        let mut send = connected_many(&["a"]);
        command(&mut send, "email.send", json!({"document": path}));
        finish(&mut send);
        let draft = send
            .take_work()
            .into_iter()
            .find_map(|w| match w {
                Work::Write {
                    write: crate::commands::Write::Send { draft },
                    ..
                } => Some(draft),
                _ => None,
            })
            .expect("a send, never a compose");
        assert_eq!(draft.attachments, [attachment]);
        assert_eq!(draft.subject, "Changed document");
        let mime = String::from_utf8(jmap::build_mime(&draft).unwrap()).unwrap();
        assert!(mime.contains("Reply-To: ann@a.example.invalid\r\n"));
        assert!(mime.contains("References: <first@example.invalid> <second@example.invalid>\r\n"));
        assert!(mime.contains("Content-Type: text/plain"));
        assert!(!mime.contains("Content-Type: text/html"));
        assert!(mime.contains("Y2hhbmdlZCBhdCBzZW5kIHRpbWU="));
    }

    #[test]
    fn open_replacement_cancel_and_stale_reads_preserve_the_draft() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.md");
        std::fs::write(&path, "---\nmail-subject: Loaded\n---\nBody").unwrap();
        let mut state = connected_many(&["a"]);
        state.open_compose(
            Some("a"),
            jmap::Composed {
                subject: "Keep".into(),
                ..Default::default()
            },
        );
        state.apply_ui(crate::view::UiEvent::ComposeField(
            crate::state::ComposeField::Subject,
            "Keep edited".into(),
        ));
        let old_id = state.compose.as_ref().unwrap().draft_id.clone();
        state
            .read_document("a".into(), path.clone(), None, false, false)
            .unwrap();
        finish(&mut state);
        assert!(state.documents.replace.is_some());
        state.confirm_document_replace(false);
        assert_eq!(state.compose.as_ref().unwrap().draft.subject, "Keep edited");
        state
            .read_document("a".into(), path.clone(), None, false, false)
            .unwrap();
        state.apply_ui(crate::view::UiEvent::ComposeField(
            crate::state::ComposeField::Subject,
            "Newer".into(),
        ));
        finish(&mut state);
        assert!(state.documents.replace.is_none());
        assert_eq!(state.compose.as_ref().unwrap().draft.subject, "Newer");
        state
            .read_document("a".into(), path, None, false, false)
            .unwrap();
        finish(&mut state);
        state.confirm_document_replace(true);
        assert_eq!(state.compose.as_ref().unwrap().draft.subject, "Loaded");
        assert_eq!(state.compose.as_ref().unwrap().body.contents(), "Body");
        assert!(
            state.restore.is_empty(),
            "Replace must not offer the discarded message"
        );
        assert!(state.take_draft_ops().iter().any(|op| matches!(op,
            crate::drafts::DraftOp::Remove(id) if id == &old_id)));
    }

    #[test]
    fn opening_over_an_untouched_signature_does_not_ask_to_replace() {
        let mut state = connected_many(&["a"]);
        state.accounts[0].entry.signature = "Ann's signature".into();
        assert!(state.open_compose(Some("a"), jmap::Composed::default()));
        assert!(!state.compose.as_ref().unwrap().body.contents().is_empty());
        assert!(!state.compose.as_ref().unwrap().keepable());
        state
            .read_document("a".into(), "/tmp/mail.md".into(), None, false, false)
            .unwrap();
        let ticket = *state.documents.pending.keys().next().unwrap();
        state.document_done(
            ticket,
            Answer::Read(Ok(MailDocument {
                subject: "Opened".into(),
                ..Default::default()
            })),
        );
        assert!(state.documents.replace.is_none());
        assert_eq!(state.compose.as_ref().unwrap().draft.subject, "Opened");
    }

    #[test]
    fn removing_the_compose_account_cancels_document_modals_and_late_answers() {
        for modal in ["folder", "chooser", "replace"] {
            let mut state = connected_many(&["a", "b"]);
            assert!(state.open_compose(Some("a"), jmap::Composed::default()));
            state.save_document_as();
            let folder_ticket = state.documents.folder.as_ref().unwrap().0;
            if modal == "chooser" {
                state.document_done(folder_ticket, Answer::Folder("/tmp".into()));
            }
            if modal == "replace" {
                state.apply_ui(crate::view::UiEvent::ComposeField(
                    crate::state::ComposeField::Subject,
                    "Edited".into(),
                ));
                state
                    .read_document("a".into(), "/tmp/mail.md".into(), None, false, false)
                    .unwrap();
                let ticket = *state.documents.pending.keys().next().unwrap();
                state.document_done(ticket, Answer::Read(Ok(MailDocument::default())));
                assert!(state.documents.replace.is_some());
            }
            assert!(state.documents.modal());
            state
                .read_document("b".into(), "/tmp/send.md".into(), Some(72), true, false)
                .unwrap();
            let send_ticket = *state
                .documents
                .pending
                .iter()
                .find(|(_, r)| r.send)
                .unwrap()
                .0;
            state.set_settings(crate::state::tests::settings(&["b"]));
            assert!(state.compose.is_none());
            assert!(!state.documents.modal(), "{modal}");
            assert!(!state.documents.opening(), "{modal}");
            assert!(
                state.documents.pending.contains_key(&send_ticket),
                "send survives"
            );
            state.document_done(folder_ticket, Answer::Folder("/tmp".into()));
            assert!(
                !state.documents.modal(),
                "late folder must not reopen a chooser"
            );
            command(&mut state, "email.compose", json!({"account": "b"}));
            assert_eq!(state.take_replies()[0].rc, 0, "compose must not stay BUSY");
        }
    }

    #[test]
    fn failed_save_reports_after_an_edit_or_close_and_stale_success_keeps_errors() {
        for close in [false, true] {
            let mut state = connected_many(&["a"]);
            state.open_compose(Some("a"), jmap::Composed::default());
            state.write_document("/tmp/mail.md".into(), false);
            let ticket = *state.documents.saving.keys().next().unwrap();
            if close {
                state.take_compose();
                state.close_out();
            } else {
                state.apply_ui(crate::view::UiEvent::ComposeField(
                    crate::state::ComposeField::Subject,
                    "Edited".into(),
                ));
            }
            state.document_done(
                ticket,
                Answer::Saved(Err((code::DOCUMENT_IO, "disk full".into()))),
            );
            if close {
                assert_eq!(state.status, "disk full");
            } else {
                assert_eq!(
                    state.compose.as_ref().unwrap().error.as_deref(),
                    Some("disk full")
                );
                state.write_document("/tmp/mail.md".into(), false);
                let ticket = *state.documents.saving.keys().next().unwrap();
                state.compose.as_mut().unwrap().revision += 1;
                state.document_done(ticket, Answer::Saved(Ok(())));
                assert_eq!(
                    state.compose.as_ref().unwrap().error.as_deref(),
                    Some("disk full")
                );
            }
        }
    }

    #[test]
    fn replacement_write_failure_preserves_the_old_file_and_cleans_the_sibling() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.md");
        std::fs::write(&path, "old document").unwrap();
        let inode = std::fs::metadata(&path).unwrap().ino();
        let result = save_file(&path, true, |file| {
            file.write_all(b"partial")?;
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "old document");
            Err(std::io::Error::other("disk full"))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old document");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        save_file(&path, true, |file| file.write_all(b"new document")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new document");
        let metadata = std::fs::metadata(&path).unwrap();
        assert_ne!(metadata.ino(), inode, "replacement must use a new inode");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert!(save_file(&path, false, |_| panic!("must not overwrite")).is_err());
        let link = dir.path().join("link.md");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(save_file(&link, true, |_| panic!("must not follow links")).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn drops_open_documents_and_plain_markdown_falls_back_to_an_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.md");
        std::fs::write(&path, "---\nmail-subject: Dropped\n---\n").unwrap();
        let mut state = connected_many(&["a"]);
        state.drop_files(vec![path.clone()]);
        finish(&mut state);
        assert_eq!(state.compose.as_ref().unwrap().draft.subject, "Dropped");
        assert!(
            !state
                .take_work()
                .iter()
                .any(|w| matches!(w, Work::Send { .. } | Work::Write { .. }))
        );
        std::fs::write(&path, "# Ordinary Markdown").unwrap();
        state.drop_files(vec![path.clone()]);
        finish(&mut state);
        assert_eq!(
            state.compose.as_ref().unwrap().draft.attachments.as_slice(),
            std::slice::from_ref(&path)
        );
        let mut idle = connected_many(&["a"]);
        idle.drop_files(vec![path]);
        finish(&mut idle);
        assert!(idle.compose.is_none());
        assert!(idle.status.contains("framing"));
    }

    #[test]
    fn export_waits_for_the_document_attachment_paths_to_be_installed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.md");
        let output = dir.path().join("output.md");
        let attachment = dir.path().join("note.txt");
        std::fs::write(&attachment, "note").unwrap();
        let doc = MailDocument {
            attachments: vec![attachment],
            ..Default::default()
        };
        std::fs::write(&path, maildoc::write(&doc).unwrap()).unwrap();
        let mut state = connected_many(&["a"]);
        command(&mut state, "email.compose", json!({"document": path}));
        for work in state.take_work() {
            if let Work::MailDocument(work) = work {
                let (ticket, answer) = work.perform();
                state.document_done(ticket, answer);
            }
        }
        state.write_document(output.clone(), false);
        let work = state.take_work();
        assert!(
            !work
                .iter()
                .any(|w| matches!(w, Work::MailDocument(DocumentWork::Save { .. })))
        );
        for item in work {
            state.queue_work(item);
        }
        state.finish_compose_inspections();
        state.write_document(output.clone(), false);
        finish(&mut state);
        assert_eq!(read(&output).unwrap().attachments, doc.attachments);
    }

    #[test]
    fn save_is_deferred_refuses_controls_and_never_overwrites_an_unapproved_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.md");
        let mut state = connected_many(&["a"]);
        state.open_compose(
            Some("a"),
            jmap::Composed {
                subject: "Saved".into(),
                body: "Text\n\n".into(),
                ..Default::default()
            },
        );
        state.write_document(path.clone(), false);
        assert!(!path.exists());
        finish(&mut state);
        assert_eq!(read(&path).unwrap().body, "Text");
        std::fs::write(&path, "keep").unwrap();
        state.write_document(path.clone(), false);
        finish(&mut state);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep");
        state.write_document(path.clone(), true);
        finish(&mut state);
        assert_eq!(read(&path).unwrap().subject, "Saved");
        state.compose.as_mut().unwrap().draft.subject = "bad\u{0007}".into();
        state.write_document(path.clone(), true);
        finish(&mut state);
        assert!(
            state
                .compose
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("control")
        );
        assert_eq!(read(&path).unwrap().subject, "Saved");
        state.compose.as_mut().unwrap().draft.subject = "Recovered".into();
        state.write_document(path.clone(), true);
        finish(&mut state);
        assert!(state.compose.as_ref().unwrap().error.is_none());
        assert_eq!(read(&path).unwrap().subject, "Recovered");
    }
}
