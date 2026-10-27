// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Mail document effects and fenced state transitions. Opening never sends.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

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
    let file = std::fs::OpenOptions::new().read(true)
        .custom_flags(libc::O_NONBLOCK).open(path).map_err(io)?;
    if !file.metadata().map_err(io)?.is_file() {
        return Err((code::DOCUMENT_IO, crate::label("document-not-file")));
    }
    let mut text = Vec::new();
    file.take(maildoc::MAX_DOCUMENT_BYTES as u64 + 1).read_to_end(&mut text).map_err(io)?;
    if text.len() > maildoc::MAX_DOCUMENT_BYTES {
        return Err(refusal(Refusal::TooLarge { bytes: text.len() }));
    }
    let text = String::from_utf8(text).map_err(|error| (code::DOCUMENT_ENCODING, error.to_string()))?;
    maildoc::parse(&text).map_err(refusal)
}

#[derive(Clone, Debug, PartialEq)]
pub enum DocumentWork {
    Folder { ticket: u64 },
    Read { ticket: u64, path: PathBuf },
    Save { ticket: u64, path: PathBuf, replace: bool, document: MailDocument },
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
            Self::Save { ticket, path, replace, document } => {
                let result = maildoc::write(&document).map_err(refusal).and_then(|text| {
                    // A destination which appeared after the chooser checked it
                    // must never be overwritten without the replacement question.
                    let mut options = std::fs::OpenOptions::new();
                    options.write(true).mode(0o600).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
                    if replace { options.truncate(true); } else { options.create_new(true); }
                    let mut file = options.open(&path).map_err(|e| (code::DOCUMENT_IO, e.to_string()))?;
                    if !file.metadata().map_err(|e| (code::DOCUMENT_IO, e.to_string()))?.is_file() {
                        return Err((code::DOCUMENT_IO, crate::label("document-not-file")));
                    }
                    file.write_all(text.as_bytes()).and_then(|()| file.sync_all())
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
    pub fn opening(&self) -> bool { self.pending.values().any(|r| !r.send) }
    pub fn modal(&self) -> bool { self.chooser.is_some() || self.replace.is_some() || self.folder.is_some() }
}

impl State {
    fn document_fence(&self) -> Option<(String, u64)> {
        self.compose.as_ref().map(|c| (c.draft_id.clone(), c.revision))
    }

    fn document_error(&mut self, error: Failure) {
        if let Some(compose) = &mut self.compose { compose.error = Some(error.1); }
        else { self.status = error.1; }
    }

    pub fn read_document(&mut self, account: String, path: PathBuf, reply: Option<u64>, send: bool, dropped: bool) -> Result<()> {
        if !path.is_absolute() { return Err((code::DOCUMENT_PATH, crate::label("document-absolute-path"))); }
        if self.documents.pending.len() >= 8 || (!send && (self.documents.opening() || self.compose.as_ref().is_some_and(|c| c.busy))) {
            return Err((code::BUSY, crate::label("compose-open")));
        }
        let generation = self.account(&account).ok_or_else(|| (code::UNKNOWN_ACCOUNT, crate::label("no-account")))?.generation;
        let ticket = self.ticket();
        self.documents.pending.insert(ticket, Request { account, generation,
            fence: self.document_fence(), reply, send, dropped, path: path.clone() });
        self.queue_work(Work::MailDocument(DocumentWork::Read { ticket, path }));
        Ok(())
    }

    pub fn open_document(&mut self) { self.choose_document(false); }
    pub fn save_document_as(&mut self) { self.choose_document(true); }

    fn choose_document(&mut self, save: bool) {
        if self.compose.as_ref().is_none_or(|c| c.busy) || self.documents.modal() || self.documents.opening() { return; }
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
            toolkit::file_chooser::FileChooser::save(directory, crate::label("document-save-title"), crate::label("document-save"))
        } else {
            toolkit::file_chooser::FileChooser::single(directory, crate::label("document-open-title"), crate::label("document-open"))
        });
    }

    /// Called by the shell after drawing; I/O remains queued as effects.
    pub fn document_chooser_ui(&mut self, ctx: &egui::Context) {
        if self.compose.is_none() { self.documents.chooser = None; }
        let outcome = self.documents.chooser.as_mut().and_then(|c| c.show(ctx));
        if let Some(outcome) = outcome {
            self.documents.last_directory = self.documents.chooser.as_ref().map(|c| c.directory().to_owned());
            self.documents.chooser = None;
            match outcome {
                toolkit::file_chooser::Outcome::Picked(paths) => {
                    if let Some(path) = paths.into_iter().next() {
                        let account = self.compose.as_ref().map(|c| c.account.clone()).unwrap_or_default();
                        if let Err(error) = self.read_document(account, path, None, false, false) { self.document_error(error); }
                    }
                }
                toolkit::file_chooser::Outcome::Save { path, replace } => self.write_document(path, replace),
                toolkit::file_chooser::Outcome::Cancelled => {}
            }
            ctx.request_repaint();
        }
    }

    pub fn write_document(&mut self, path: PathBuf, replace: bool) {
        let Some(compose) = self.compose.as_ref().filter(|c| !c.busy) else { return; };
        let addresses = |text: &str| jmap::parse_addresses(text)
            .map(|a| a.iter().map(jmap::format_address).collect::<Vec<_>>())
            .map_err(|e| (code::DOCUMENT_INVALID_VALUE, e.to_string()));
        let document = (|| Ok(MailDocument {
            to: addresses(&compose.to_text)?, cc: addresses(&compose.cc_text)?, bcc: addresses(&compose.bcc_text)?,
            from: Some(compose.draft.from.clone()), reply_to: compose.draft.reply_to.clone(),
            subject: compose.draft.subject.clone(), body: compose.body.contents().trim_end().to_owned(),
            in_reply_to: compose.draft.in_reply_to.clone(), references: compose.draft.references.clone(),
            attachments: compose.draft.attachments.clone(),
        }))();
        match document {
            Ok(document) => {
                let ticket = self.ticket();
                self.documents.saving.insert(ticket, self.document_fence());
                self.queue_work(Work::MailDocument(DocumentWork::Save { ticket, path, replace, document }));
            }
            Err(error) => self.document_error(error),
        }
    }

    pub fn drop_files(&mut self, paths: Vec<PathBuf>) {
        let mut attachments = Vec::new();
        for path in paths {
            if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md")) {
                let account = self.compose.as_ref().map(|c| c.account.clone()).or_else(|| self.compose_account());
                let result = account.ok_or_else(|| (code::UNKNOWN_ACCOUNT, crate::label("no-account")))
                    .and_then(|account| self.read_document(account, path, None, false, true));
                if let Err(error) = result { self.document_error(error); }
            } else { attachments.push(path); }
        }
        if !attachments.is_empty() && self.compose.is_some()
            && let Err(error) = self.add_compose_attachments(&attachments) { self.document_error(error); }
    }

    pub fn document_done(&mut self, ticket: u64, answer: Answer) {
        match answer {
            Answer::Folder(directory) => {
                if let Some((expected, save, fence)) = self.documents.folder.take()
                    && expected == ticket && fence == self.document_fence() {
                    self.document_chooser(directory, save);
                }
            }
            Answer::Saved(result) => {
                if self.documents.saving.remove(&ticket).is_some_and(|f| f == self.document_fence())
                    && let Err(error) = result { self.document_error(error); }
            }
            Answer::Read(result) => {
                let Some(request) = self.documents.pending.get(&ticket).cloned() else { return; };
                let result = if self.account(&request.account).is_none_or(|v| v.generation != request.generation) {
                    Err((code::STALE, crate::label("not-connected")))
                } else if !request.send && (request.fence != self.document_fence() || self.compose.as_ref().is_some_and(|c| c.busy)) {
                    Err((code::SUPERSEDED, crate::label("document-superseded")))
                } else { result };
                match result {
                    Ok(document) => {
                        // Check identity before offering a replacement.
                        if let Err(error) = composed(self, &request.account, &document) {
                            self.finish_document_error(ticket, error);
                        } else if !request.send && request.reply.is_none() && self.compose.as_ref().is_some_and(|c| {
                            !c.to_text.is_empty() || !c.cc_text.is_empty() || !c.bcc_text.is_empty()
                                || !c.draft.subject.is_empty() || !c.body.contents().is_empty() || !c.draft.attachments.is_empty()
                        }) {
                            self.documents.replace = Some((ticket, document));
                        } else { self.install_document(ticket, document); }
                    }
                    Err(error) => {
                        if request.dropped && request.fence.is_some() && request.fence == self.document_fence()
                            && !matches!(error.0, code::STALE | code::SUPERSEDED) {
                            self.documents.pending.remove(&ticket);
                            if let Err(error) = self.add_compose_attachments(&[request.path]) { self.document_error(error); }
                        } else { self.finish_document_error(ticket, error); }
                    }
                }
            }
        }
    }

    fn finish_document_error(&mut self, ticket: u64, error: Failure) {
        if let Some(request) = self.documents.pending.remove(&ticket) {
            if let Some(id) = request.reply { self.refuse(id, error.0, &error.1); }
            else { self.document_error(error); }
        }
    }

    pub fn confirm_document_replace(&mut self, replace: bool) {
        if let Some((ticket, document)) = self.documents.replace.take() {
            if replace { self.install_document(ticket, document); }
            else { self.documents.pending.remove(&ticket); }
        }
    }

    fn install_document(&mut self, ticket: u64, document: MailDocument) {
        let Some(request) = self.documents.pending.remove(&ticket) else { return; };
        let result = (|| {
            if !request.send && request.fence != self.document_fence() {
                return Err((code::SUPERSEDED, crate::label("document-superseded")));
            }
            let (draft, identity) = composed(self, &request.account, &document)?;
            if request.send {
                if self.account(&request.account).is_none_or(|v| v.phase != crate::state::Phase::Ready) {
                    return Err((code::NOT_CONNECTED, crate::label("not-connected")));
                }
                if draft.to.is_empty() || draft.subject.trim().is_empty() {
                    return Err((code::INVALID_ARGUMENT, crate::label("document-send-required")));
                }
                self.write_server(request.account.clone(), request.reply, crate::commands::Write::Send { draft });
            } else {
                let paths = draft.attachments.clone();
                self.open_compose(Some(&request.account), draft.clone());
                let compose = self.compose.as_mut().ok_or_else(|| (code::UNKNOWN_ACCOUNT, crate::label("no-account")))?;
                compose.draft.from = draft.from;
                compose.draft.from_name = draft.from_name;
                compose.draft.reply_to = draft.reply_to;
                compose.identity = identity;
                compose.set_body(&document.body);
                if let Some(error) = self.close_failed_compose() { return Err((code::DOCUMENT_INVALID_VALUE, error)); }
                self.inspect_compose_attachments(&paths, request.reply, true)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            if let Some(id) = request.reply { self.refuse(id, error.0, &error.1); }
            else { self.document_error(error); }
        }
    }
}

/// Account-owned addresses only, including Reply-To. Display names on a
/// document address do not let it bypass ownership of the parsed mailbox.
pub fn identity(state: &State, account: &str, from: Option<&str>, reply_to: Option<&str>) -> Result<settings::email::Identity> {
    let senders = state.senders(account);
    let mailbox = |value: &str| -> Result<String> {
        let addresses = jmap::parse_addresses(value).map_err(|e| (code::IDENTITY_MISMATCH, e.to_string()))?;
        if addresses.len() != 1 { return Err((code::IDENTITY_MISMATCH, crate::label("document-identity"))); }
        Ok(addresses[0].email.clone())
    };
    let from = from.map(mailbox).transpose()?;
    let sender = senders.iter().find(|s| from.as_ref().is_none_or(|f| s.address.eq_ignore_ascii_case(f)))
        .cloned().ok_or_else(|| (code::IDENTITY_MISMATCH, crate::label("document-identity")))?;
    if let Some(reply) = reply_to {
        let reply = mailbox(reply)?;
        if !senders.iter().any(|s| s.address.eq_ignore_ascii_case(&reply)) {
            return Err((code::IDENTITY_MISMATCH, crate::label("document-identity")));
        }
    }
    Ok(sender)
}

fn composed(state: &State, account: &str, doc: &MailDocument) -> Result<(jmap::Composed, String)> {
    let sender = identity(state, account, doc.from.as_deref(), doc.reply_to.as_deref())?;
    let addresses = |values: &[String]| jmap::parse_addresses(&values.join(", "))
        .map_err(|e| (code::DOCUMENT_INVALID_VALUE, e.to_string()));
    Ok((jmap::Composed {
        from: sender.address, from_name: sender.name, reply_to: doc.reply_to.clone(),
        to: addresses(&doc.to)?, cc: addresses(&doc.cc)?, bcc: addresses(&doc.bcc)?,
        subject: doc.subject.clone(), body: doc.body.clone(), attachments: doc.attachments.clone(),
        in_reply_to: doc.in_reply_to.clone(), references: doc.references.clone(),
    }, sender.id))
}
