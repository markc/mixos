// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Drafts: a compose's content kept in its account's Drafts mailbox.
//!
//! An edit arms one timer ([`Work::DraftTimer`], [`DRAFT_DELAY`]), and only
//! the timer of the latest edit saves, so a burst of typing saves once, five
//! seconds after it stops. Nothing runs while nobody types. A save is one
//! `Email/set` (`jmap::Client::save_draft`): the new draft is created in
//! Drafts and the one it replaces is destroyed in the same call. At most one
//! save is in flight per compose. Edits made meanwhile wait as `queued` and
//! go when it lands, so the newest content always follows.
//!
//! Each save is an operation ticket ([`DraftOp`]), and its answer finds its
//! compose by id. When that compose has gone (sent, discarded, its account
//! removed, its From moved to another account), the draft it made is
//! destroyed, so nothing is left behind.
//!
//! Autosave runs only when the compose has content, `email.compose.autosave`
//! is on and the account has a Drafts mailbox. Save draft (the close prompt,
//! `compose.save_draft`, `email.draft.save`) saves on demand. A send that
//! goes destroys the draft, and so does Discard. Opening a draft resumes it
//! in compose, and later saves replace it.

use jmap::{Composed, SavedDraft};
use serde_json::{Value, json};

use super::{ComposeState, State, Tag, code};
use crate::verbs::Write;
use crate::work::Work;

/// How long after the last edit a draft is saved.
pub const DRAFT_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

/// Where a compose's draft stands on its account's server.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Autosave {
    /// The saved draft's id, once one is confirmed (or the draft resumed).
    pub draft_id: Option<String>,
    /// Edits made so far. A save covers the edits made when it was sent.
    pub edits: u64,
    /// The edits the last confirmed save covered.
    pub saved: u64,
    /// The save in flight: its operation ticket and the edits it covers.
    pub flight: Option<(u64, u64)>,
    /// Edits made since the save in flight was sent: saved when it lands.
    pub queued: bool,
    /// Bus callers waiting for the queued save.
    pub queued_replies: Vec<u64>,
    /// When the last save landed, as `hh:mm`.
    pub saved_at: Option<String>,
    /// Why the last save failed.
    pub error: Option<String>,
    /// Save draft from the close prompt: the dialog closes once every edit
    /// is saved.
    pub close_after: bool,
}

impl Autosave {
    /// Edits no confirmed save covers yet.
    pub fn unsaved(&self) -> bool {
        self.saved < self.edits
    }

    /// The state's name on the Bus.
    pub fn name(&self) -> &'static str {
        if self.flight.is_some() {
            "saving"
        } else if self.error.is_some() {
            "failed"
        } else if self.unsaved() {
            "unsaved"
        } else if self.draft_id.is_some() {
            "saved"
        } else {
            "none"
        }
    }
}

/// A draft save in flight.
#[derive(Clone, Debug, PartialEq)]
pub struct DraftOp {
    /// The compose it saves.
    pub compose: u64,
    /// The account it was sent to.
    pub account: String,
    /// The edits it covers.
    pub edits: u64,
    /// Bus callers waiting for it.
    pub replies: Vec<u64>,
}

/// The close prompt's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseChoice {
    /// Save the draft, then close.
    Save,
    /// Close, destroying any saved draft.
    Discard,
    /// Keep composing.
    Cancel,
}

/// A refusal for a Bus caller: its code and message.
pub type Refused = (&'static str, String);

/// The compose's content as a draft keeps it: the recipient fields as
/// typed, whatever parses and whatever does not.
fn draft_of(compose: &ComposeState) -> Composed {
    Composed {
        to: jmap::draft_addresses(&compose.to_text),
        cc: jmap::draft_addresses(&compose.cc_text),
        bcc: jmap::draft_addresses(&compose.bcc_text),
        ..compose.draft.clone()
    }
}

impl State {
    /// `account`'s Drafts mailbox id.
    pub fn drafts_mailbox_id(&self, account: &str) -> Option<String> {
        self.account(account)?
            .mail
            .by_role("drafts")
            .map(|m| m.id.clone())
    }

    /// Whether the compose holds anything worth keeping: a recipient, a
    /// subject, or a body beyond the account's signature.
    pub fn compose_has_content(&self, compose: &ComposeState) -> bool {
        let body = compose.draft.body.trim();
        [
            &compose.to_text,
            &compose.cc_text,
            &compose.bcc_text,
            &compose.draft.subject,
        ]
        .iter()
        .any(|field| !field.trim().is_empty())
            || (!body.is_empty() && body != self.signed_body(&compose.account).trim())
    }

    /// The dialog's quiet save line: saving, saved at, or why not.
    pub fn draft_line(&self) -> Option<String> {
        let compose = self.compose.as_ref()?;
        let save = &compose.save;
        if save.flight.is_some() {
            return Some(crate::label("draft-saving"));
        }
        if let Some(error) = &save.error {
            return Some(crate::label_with("draft-not-saved", &[("error", error)]));
        }
        if let Some(at) = &save.saved_at {
            return Some(crate::label_with("draft-saved", &[("time", at)]));
        }
        // Autosave is on but cannot run: say why, once the mailboxes are in.
        let mailboxes = self
            .account(&compose.account)
            .is_some_and(|v| !v.mail.mailboxes.is_empty());
        if self.settings.compose.autosave
            && mailboxes
            && self.drafts_mailbox_id(&compose.account).is_none()
        {
            let why = crate::label("no-drafts-mailbox");
            return Some(crate::label_with("draft-not-saved", &[("error", &why)]));
        }
        None
    }

    /// An edit to the open compose: counted, and with autosave on, the
    /// timer of this edit armed (only the latest one's saves).
    pub(super) fn note_edit(&mut self) {
        let autosave = self.settings.compose.autosave;
        let Some(compose) = &mut self.compose else {
            return;
        };
        compose.save.edits += 1;
        if autosave {
            let (compose, edits) = (compose.id, compose.save.edits);
            self.queue(Work::DraftTimer { compose, edits });
        }
    }

    /// Edit timer `edits` of compose `compose` ran out: it saves if no
    /// later edit has armed another.
    pub fn draft_due(&mut self, compose: u64, edits: u64) {
        let latest = self
            .compose
            .as_ref()
            .is_some_and(|c| c.id == compose && c.save.edits == edits);
        if latest && self.autosaving() {
            let _ = self.save_draft(None);
        }
    }

    /// Whether the open compose autosaves now: the setting on, content, a
    /// Drafts mailbox, and no send in flight.
    fn autosaving(&self) -> bool {
        self.settings.compose.autosave
            && self.compose.as_ref().is_some_and(|c| {
                !c.busy
                    && c.save.unsaved()
                    && self.compose_has_content(c)
                    && self.drafts_mailbox_id(&c.account).is_some()
            })
    }

    /// Save the open compose now, or as soon as the save in flight lands.
    /// `reply`, a Bus request, hears once the edits made so far are saved.
    fn save_draft(&mut self, reply: Option<u64>) -> Result<(), Refused> {
        let Some(compose) = &self.compose else {
            return Err((code::NOT_FOUND, crate::label("no-compose")));
        };
        if compose.busy {
            return Err((code::BUSY, crate::label("sending")));
        }
        if self.drafts_mailbox_id(&compose.account).is_none() {
            return Err((code::REFUSED, crate::label("no-drafts-mailbox")));
        }
        let save = &compose.save;
        if !save.unsaved()
            && let Some(id) = &save.draft_id
        {
            let body = json!({ "id": id, "compose": compose.id });
            if let Some(reply) = reply {
                self.reply(reply, 0, body);
            }
            return Ok(());
        }
        match save.flight {
            // The save in flight covers every edit: wait for it.
            Some((op, edits)) if edits >= save.edits => {
                if let Some(reply) = reply {
                    self.outstanding.insert(reply, true);
                    if let Some(op) = self.draft_ops.get_mut(&op) {
                        op.replies.push(reply);
                    }
                }
            }
            Some(_) => {
                if let Some(reply) = reply {
                    // Not sent yet: cancelled, not unknown, if Email closes.
                    self.outstanding.insert(reply, false);
                }
                if let Some(compose) = &mut self.compose {
                    compose.save.queued = true;
                    compose.save.queued_replies.extend(reply);
                }
            }
            None => self.start_save(reply.into_iter().collect()),
        }
        Ok(())
    }

    /// Send the open compose's content as a draft, replacing its saved one.
    fn start_save(&mut self, replies: Vec<u64>) {
        let op = self.ticket();
        let Some(compose) = &self.compose else {
            return;
        };
        let Some(mailbox) = self.drafts_mailbox_id(&compose.account) else {
            return;
        };
        let (account, draft) = (compose.account.clone(), draft_of(compose));
        let replaces = compose.save.draft_id.clone();
        let edits = compose.save.edits;
        let id = compose.id;
        for reply in &replies {
            self.outstanding.insert(*reply, true);
        }
        if let Some(compose) = &mut self.compose {
            compose.save.flight = Some((op, edits));
            compose.save.queued = false;
        }
        self.draft_ops.insert(
            op,
            DraftOp {
                compose: id,
                account: account.clone(),
                edits,
                replies,
            },
        );
        self.queue(Work::SaveDraft {
            account,
            op,
            draft,
            mailbox,
            replaces,
        });
    }

    /// Draft save `op` finished. Its compose, if still open on the same
    /// account, takes the new id and sends any queued edits; otherwise
    /// the draft it made is destroyed.
    pub(super) fn draft_saved(
        &mut self,
        tag: &Tag,
        op: u64,
        result: Result<SavedDraft, jmap::Error>,
    ) {
        let Some(done) = self.draft_ops.remove(&op) else {
            return;
        };
        let live = self
            .compose
            .as_ref()
            .is_some_and(|c| c.id == done.compose && c.account == done.account);
        let saved = match result {
            Ok(saved) => saved,
            Err(error) => {
                let (code, message) = crate::verbs::server_refusal(&error);
                for reply in &done.replies {
                    self.refuse(*reply, code, &message);
                }
                if live && let Some(compose) = &mut self.compose {
                    compose.save.flight = None;
                    compose.save.error = Some(message);
                    // The person chose to keep it: the dialog stays, saying why.
                    compose.save.close_after = false;
                }
                return self.after_save();
            }
        };
        if !live {
            // Sent, discarded or moved on: the draft it made must not stay.
            self.destroy_draft(tag.account.clone(), saved.id);
            let message = crate::label("draft-compose-closed");
            for reply in &done.replies {
                self.refuse(*reply, code::CANCELLED, &message);
            }
            return;
        }
        let at = chrono::Local::now().format("%H:%M").to_string();
        let mut sending = None;
        if let Some(compose) = &mut self.compose {
            compose.save.draft_id = Some(saved.id.clone());
            compose.save.saved = compose.save.saved.max(done.edits);
            compose.save.flight = None;
            compose.save.error = None;
            compose.save.saved_at = Some(at);
            sending = compose.sending;
        }
        // A send went out meanwhile: it destroys this draft, not the old one.
        if let Some(ticket) = sending {
            self.send_drafts
                .insert(ticket, (done.account.clone(), saved.id.clone()));
        }
        if let Some(kept) = &saved.kept {
            self.status = crate::label_with("draft-old-kept", &[("error", kept)]);
        }
        let body = json!({ "id": saved.id, "compose": done.compose });
        for reply in &done.replies {
            self.reply(*reply, 0, body.clone());
        }
        self.after_save();
    }

    /// A save landed: queued edits go now, and a Save draft from the close
    /// prompt closes the dialog once nothing is left unsaved.
    fn after_save(&mut self) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        if compose.save.queued && compose.save.flight.is_none() && !compose.busy {
            let replies = std::mem::take(&mut compose.save.queued_replies);
            return self.start_save(replies);
        }
        if compose.save.close_after
            && compose.save.flight.is_none()
            && !compose.save.unsaved()
            && compose.save.error.is_none()
        {
            self.status = crate::label("draft-kept");
            self.drop_compose();
        }
    }

    /// `compose.save_draft`: save the open compose now.
    pub fn save_draft_now(&mut self) {
        if let Err((_, message)) = self.save_draft(None)
            && let Some(compose) = &mut self.compose
        {
            compose.save.error = Some(message);
        }
    }

    /// Whether Save draft can run: a compose open, not sending, with a
    /// Drafts mailbox to save it in.
    pub fn draft_savable(&self) -> bool {
        self.compose
            .as_ref()
            .is_some_and(|c| !c.busy && self.drafts_mailbox_id(&c.account).is_some())
    }

    /// `email.draft.save`: save compose `compose` (the open one when
    /// `None`) and answer `reply` with the draft's id once it is saved.
    pub fn draft_save_bus(&mut self, reply: u64, compose: Option<u64>) -> Result<(), Refused> {
        let open = self.compose.as_ref().map(|c| c.id);
        if open.is_none() || (compose.is_some() && compose != open) {
            return Err((code::NOT_FOUND, crate::label("no-compose")));
        }
        self.save_draft(Some(reply))
    }

    /// Close asked for (Cancel, Esc, `email.execute`): with unsaved work,
    /// the prompt opens; otherwise the dialog closes and a saved draft stays.
    pub(super) fn ask_close(&mut self) {
        let Some(compose) = &self.compose else {
            return;
        };
        let touched = compose.save.edits > 0
            && (self.compose_has_content(compose) || compose.save.draft_id.is_some());
        if touched && !compose.busy {
            if let Some(compose) = &mut self.compose {
                compose.closing = true;
                compose.suggest = None;
            }
        } else {
            self.drop_compose();
        }
    }

    /// The close prompt's answer.
    pub(super) fn close_choice(&mut self, choice: CloseChoice) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        compose.closing = false;
        match choice {
            CloseChoice::Cancel => {}
            CloseChoice::Save => {
                compose.save.close_after = true;
                match self.save_draft(None) {
                    Ok(()) => self.after_save(),
                    Err((_, message)) => {
                        if let Some(compose) = &mut self.compose {
                            compose.save.close_after = false;
                            compose.save.error = Some(message);
                        }
                    }
                }
            }
            CloseChoice::Discard => {
                let draft = compose.save.draft_id.take();
                let account = compose.account.clone();
                if let Some(id) = draft {
                    self.destroy_draft(account, id);
                }
                self.drop_compose();
            }
        }
    }

    /// The compose goes. Bus callers waiting on a save not yet sent hear
    /// it was cancelled; a save in flight is settled when it lands.
    pub(super) fn drop_compose(&mut self) {
        if let Some(compose) = self.compose.take() {
            let message = crate::label("draft-compose-closed");
            for reply in compose.save.queued_replies {
                self.refuse(reply, code::CANCELLED, &message);
            }
        }
    }

    /// Destroy draft `id` on `account`'s server, for good (not to Trash).
    pub(super) fn destroy_draft(&mut self, account: String, id: String) {
        let write = Write::Delete {
            ids: vec![id],
            destroy: true,
        };
        self.write_server(account, None, write);
    }

    /// Closing: the open compose's unsaved edits are saved now, inside the
    /// close's budget. Whether a save is in flight after the call.
    pub fn save_at_quit(&mut self) -> bool {
        if self.autosaving() {
            match self.compose.as_ref().and_then(|c| c.save.flight) {
                Some(_) => {
                    if let Some(compose) = &mut self.compose {
                        compose.save.queued = true;
                    }
                }
                None => self.start_save(Vec::new()),
            }
        }
        self.drafts_pending()
    }

    /// Whether a draft save is in flight.
    pub fn drafts_pending(&self) -> bool {
        !self.draft_ops.is_empty()
    }

    /// Open `account`'s draft `id` in compose, once it is read.
    pub(super) fn resume_draft(&mut self, account: &str, id: String) {
        let ticket = self.ticket();
        self.resuming = Some((ticket, account.to_owned(), id.clone()));
        self.queue(Work::ResumeDraft {
            account: account.to_owned(),
            ticket,
            id,
        });
    }

    /// A draft was read: it opens in compose (when none is open), and its
    /// later saves replace it.
    pub(super) fn resumed(
        &mut self,
        tag: &Tag,
        ticket: u64,
        result: Result<Composed, jmap::Error>,
    ) {
        let Some((_, account, id)) = self
            .resuming
            .take_if(|(t, account, _)| *t == ticket && *account == tag.account)
        else {
            return;
        };
        let draft = match result {
            Ok(draft) => draft,
            Err(error) => {
                self.status = error.to_string();
                return;
            }
        };
        if self.compose.is_some() {
            self.status = crate::label("compose-open");
            return;
        }
        // The fields show the recipients as saved: an entry that did not
        // parse comes back as typed.
        if self.open_compose(Some(&account), draft)
            && let Some(compose) = &mut self.compose
        {
            compose.save.draft_id = Some(id);
        }
    }

    /// The open compose's draft, as `email.info` shows it.
    pub(super) fn draft_info(&self, compose: &ComposeState) -> Value {
        let save = &compose.save;
        json!({
            "id": save.draft_id,
            "state": save.name(),
            "saved_at": save.saved_at,
            "error": save.error,
            "queued": save.queued,
            "autosave": self.settings.compose.autosave,
            "line": self.draft_line(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{Done, Event};
    use crate::state::ComposeField;
    use crate::state::tests::{connected_many, tag};
    use crate::view::UiEvent;
    use jmap::Mailbox;

    fn mailbox(id: &str, role: &str) -> Mailbox {
        Mailbox {
            id: id.into(),
            name: id.to_uppercase(),
            role: Some(role.into()),
            ..Mailbox::default()
        }
    }

    /// Account `a` connected, with an inbox, Drafts and Sent; a compose open.
    fn composing() -> State {
        let mut state = connected_many(&["a"]);
        let tag = tag(&state, "a");
        let boxes = vec![
            mailbox("in", "inbox"),
            mailbox("dr", "drafts"),
            mailbox("st", "sent"),
        ];
        state.apply(Event::Account(tag, Done::Mailboxes(Ok(boxes))));
        state.apply_ui(UiEvent::Compose);
        state.take_work();
        state
    }

    fn type_in(state: &mut State, field: ComposeField, text: &str) {
        state.apply_ui(UiEvent::ComposeField(field, text.into()));
    }

    fn compose_id(state: &State) -> u64 {
        state.compose.as_ref().expect("a compose").id
    }

    fn timers(work: &[Work]) -> Vec<u64> {
        work.iter()
            .filter_map(|w| match w {
                Work::DraftTimer { edits, .. } => Some(*edits),
                _ => None,
            })
            .collect()
    }

    /// The draft saves queued: (op, replaces).
    fn saves(work: &[Work]) -> Vec<(u64, Option<String>)> {
        work.iter()
            .filter_map(|w| match w {
                Work::SaveDraft { op, replaces, .. } => Some((*op, replaces.clone())),
                _ => None,
            })
            .collect()
    }

    fn destroys(work: &[Work]) -> Vec<String> {
        work.iter()
            .filter_map(|w| match w {
                Work::Write {
                    write: Write::Delete { ids, destroy: true },
                    ..
                } => Some(ids.join(",")),
                _ => None,
            })
            .collect()
    }

    fn landed(state: &mut State, op: u64, id: &str) {
        let saved = SavedDraft {
            id: id.into(),
            kept: None,
        };
        let tag = tag(state, "a");
        state.apply(Event::Account(tag, Done::DraftSaved(op, Ok(saved))));
    }

    /// Edits arm one timer each; only the latest one's saves, once.
    #[test]
    fn a_burst_of_edits_saves_once_after_the_last() {
        let mut state = composing();
        let compose = compose_id(&state);
        type_in(&mut state, ComposeField::To, "ann@example.invalid");
        type_in(&mut state, ComposeField::Subject, "Lu");
        type_in(&mut state, ComposeField::Subject, "Lunch");
        let work = state.take_work();
        assert_eq!(timers(&work), [1, 2, 3]);
        assert!(saves(&work).is_empty(), "nothing saves while typing");
        state.draft_due(compose, 1);
        state.draft_due(compose, 2);
        assert!(
            saves(&state.take_work()).is_empty(),
            "older timers are void"
        );
        state.draft_due(compose, 3);
        let work = state.take_work();
        match work.as_slice() {
            [
                Work::SaveDraft {
                    account,
                    draft,
                    mailbox,
                    replaces: None,
                    ..
                },
            ] => {
                assert_eq!((account.as_str(), mailbox.as_str()), ("a", "dr"));
                assert_eq!(draft.subject, "Lunch");
                assert_eq!(draft.to[0].email, "ann@example.invalid");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(state.compose.as_ref().unwrap().save.name(), "saving");
    }

    /// One save in flight at a time; the next one replaces only the draft
    /// the server confirmed, and a failed save leaves the confirmed one in
    /// place for the next to replace.
    #[test]
    fn each_save_replaces_the_last_confirmed_draft() {
        let mut state = composing();
        let compose = compose_id(&state);
        type_in(&mut state, ComposeField::Subject, "One");
        state.draft_due(compose, 1);
        let (op1, _) = saves(&state.take_work())[0].clone();
        // A later edit while the first is in flight waits for it.
        type_in(&mut state, ComposeField::Subject, "Two");
        state.draft_due(compose, 2);
        assert!(saves(&state.take_work()).is_empty());
        assert!(state.compose.as_ref().unwrap().save.queued);
        assert_eq!(state.compose.as_ref().unwrap().save.draft_id, None);
        landed(&mut state, op1, "E1");
        let save = &state.compose.as_ref().unwrap().save;
        assert_eq!(save.draft_id.as_deref(), Some("E1"));
        assert!(save.saved_at.is_some());
        let next = saves(&state.take_work());
        assert_eq!(next.len(), 1, "the queued edits go at once");
        let (op2, replaces) = next[0].clone();
        assert_eq!(replaces.as_deref(), Some("E1"));
        // The second save fails: E1 stays the draft the next one replaces.
        let tag = tag(&state, "a");
        let error = jmap::Error::method("serverFail", Some("disk full"));
        state.apply(Event::Account(tag, Done::DraftSaved(op2, Err(error))));
        let save = &state.compose.as_ref().unwrap().save;
        assert_eq!(save.draft_id.as_deref(), Some("E1"));
        assert_eq!(save.name(), "failed");
        assert!(state.draft_line().unwrap().contains("disk full"));
        type_in(&mut state, ComposeField::Subject, "Three");
        state.draft_due(compose, 3);
        let (op3, replaces) = saves(&state.take_work())[0].clone();
        assert_eq!(replaces.as_deref(), Some("E1"));
        landed(&mut state, op3, "E3");
        let save = &state.compose.as_ref().unwrap().save;
        assert_eq!(
            (save.draft_id.as_deref(), save.name()),
            (Some("E3"), "saved")
        );
    }

    /// A send that goes destroys the saved draft; a save still in flight
    /// when it went destroys what it made when it lands.
    #[test]
    fn a_send_destroys_the_draft() {
        let mut state = composing();
        let compose = compose_id(&state);
        type_in(&mut state, ComposeField::To, "ann@example.invalid");
        type_in(&mut state, ComposeField::Subject, "Lunch");
        state.draft_due(compose, 2);
        let (op, _) = saves(&state.take_work())[0].clone();
        landed(&mut state, op, "E1");
        state.take_work();
        type_in(&mut state, ComposeField::Body, "Noon?");
        state.draft_due(compose, 3);
        let (late, _) = saves(&state.take_work())[0].clone();
        state.apply_ui(UiEvent::SendCompose);
        let ticket = state
            .take_work()
            .into_iter()
            .find_map(|w| match w {
                Work::Send { ticket, .. } => Some(ticket),
                _ => None,
            })
            .expect("a send");
        let tag = tag(&state, "a");
        state.apply(Event::Account(tag.clone(), Done::Sent(ticket, Ok(()))));
        assert!(state.compose.is_none());
        assert_eq!(destroys(&state.take_work()), ["E1"]);
        // The save sent before the send lands after it: its draft goes too.
        landed(&mut state, late, "E2");
        assert_eq!(destroys(&state.take_work()), ["E2"]);
    }

    /// Opening a message in Drafts resumes it in compose, and later saves
    /// replace it.
    #[test]
    fn a_draft_opens_in_compose_and_its_saves_replace_it() {
        let mut state = composing();
        state.apply_ui(UiEvent::CloseCompose);
        assert!(state.compose.is_none(), "untouched: closes without asking");
        state.select_mailbox("a", "dr".into());
        state.take_work();
        let page = jmap::EmailPage {
            emails: vec![
                serde_json::from_value(json!({ "id": "E9", "subject": "Plan", "keywords": { "$draft": true, "$seen": true } }))
                    .unwrap(),
            ],
            total: 1,
        };
        let ticket = state.account("a").unwrap().tickets.page;
        let tag = tag(&state, "a");
        state.apply(Event::Account(tag.clone(), Done::Page(ticket, Ok(page))));
        state.take_work();
        assert!(state.open_email("a", "E9".into()));
        let ticket = match state.take_work().as_slice() {
            [
                Work::ResumeDraft {
                    account,
                    ticket,
                    id,
                },
            ] if account == "a" && id == "E9" => *ticket,
            other => panic!("{other:?}"),
        };
        assert!(
            state.account("a").unwrap().mail.open.is_none(),
            "not the reader"
        );
        let draft = Composed {
            to: jmap::draft_addresses("Ann <ann@example.invalid>, bo"),
            bcc: jmap::draft_addresses("dave@example.invalid"),
            subject: "Plan".into(),
            body: "Draft text".into(),
            in_reply_to: Some("<m1@example.invalid>".into()),
            ..Composed::default()
        };
        state.apply(Event::Account(
            tag,
            Done::Resumed(ticket, Box::new(Ok(draft))),
        ));
        let compose = state.compose.clone().expect("resumed in compose");
        assert_eq!(compose.to_text, "Ann <ann@example.invalid>, bo");
        assert_eq!(compose.bcc_text, "dave@example.invalid");
        assert_eq!(
            compose.draft.in_reply_to.as_deref(),
            Some("<m1@example.invalid>")
        );
        assert_eq!(compose.save.draft_id.as_deref(), Some("E9"));
        assert_eq!(compose.draft.from, "ann@a.example.invalid");
        type_in(&mut state, ComposeField::Body, "Draft text, more");
        state.draft_due(compose.id, 1);
        assert_eq!(saves(&state.take_work())[0].1.as_deref(), Some("E9"));
    }

    /// Quitting saves the unsaved edits at once; a save still unanswered
    /// when the window closes is OUTCOME_UNKNOWN to its caller.
    #[test]
    fn quitting_saves_the_draft_and_an_unanswered_save_is_unknown() {
        let mut state = composing();
        type_in(&mut state, ComposeField::Subject, "Late");
        state.take_work();
        assert!(state.save_at_quit(), "a save is in flight");
        let (op, _) = saves(&state.take_work())[0].clone();
        landed(&mut state, op, "E1");
        assert!(!state.drafts_pending());
        assert!(!state.save_at_quit(), "nothing left to save");
        // Again, with a Bus caller waiting and no answer before the close.
        type_in(&mut state, ComposeField::Subject, "Later");
        state
            .draft_save_bus(41, None)
            .expect("a save for the caller");
        assert!(state.save_at_quit());
        state.close_out();
        let replies = state.take_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].id, 41);
        assert_eq!(replies[0].body["error_code"], code::OUTCOME_UNKNOWN);
        assert!(!state.drafts_pending());
    }

    #[test]
    fn the_bus_hears_the_draft_id_once_it_is_saved() {
        let mut state = composing();
        let compose = compose_id(&state);
        type_in(&mut state, ComposeField::Subject, "Hi");
        state.draft_save_bus(7, Some(compose)).unwrap();
        assert!(state.take_replies().is_empty(), "answered once saved");
        let (op, _) = saves(&state.take_work())[0].clone();
        landed(&mut state, op, "E5");
        let reply = &state.take_replies()[0];
        assert_eq!((reply.id, reply.rc), (7, 0));
        assert_eq!(reply.body, json!({ "id": "E5", "compose": compose }));
        // Nothing new since: answered at once.
        state.draft_save_bus(8, None).unwrap();
        assert_eq!(state.take_replies()[0].body["id"], "E5");
        assert_eq!(
            state.draft_save_bus(9, Some(compose + 100)).unwrap_err().0,
            code::NOT_FOUND
        );
    }

    /// Closing with unsaved work asks; Cancel keeps it, Save draft closes
    /// once it is saved, Discard destroys the saved draft.
    #[test]
    fn closing_asks_and_each_answer_does_what_it_says() {
        let mut state = composing();
        type_in(&mut state, ComposeField::Subject, "Hi");
        state.take_work();
        state.apply_ui(UiEvent::CloseCompose);
        assert!(state.compose.as_ref().unwrap().closing);
        state.apply_ui(UiEvent::ComposeClose(CloseChoice::Cancel));
        assert!(!state.compose.as_ref().unwrap().closing);
        state.apply_ui(UiEvent::CloseCompose);
        state.apply_ui(UiEvent::ComposeClose(CloseChoice::Save));
        let (op, _) = saves(&state.take_work())[0].clone();
        assert!(state.compose.is_some(), "open until the server has it");
        landed(&mut state, op, "E1");
        assert!(state.compose.is_none());
        assert_eq!(state.status, crate::label("draft-kept"));
        // Discard: the draft it saved goes.
        state.apply_ui(UiEvent::Compose);
        let compose = compose_id(&state);
        type_in(&mut state, ComposeField::Subject, "Again");
        state.draft_due(compose, 1);
        let (op, _) = saves(&state.take_work())[0].clone();
        landed(&mut state, op, "E2");
        state.apply_ui(UiEvent::CloseCompose);
        state.apply_ui(UiEvent::ComposeClose(CloseChoice::Discard));
        assert!(state.compose.is_none());
        assert_eq!(destroys(&state.take_work()), ["E2"]);
    }

    #[test]
    fn without_drafts_or_autosave_nothing_is_saved_and_the_line_says_why() {
        let mut state = composing();
        let tag = tag(&state, "a");
        state.apply(Event::Account(
            tag,
            Done::Mailboxes(Ok(vec![mailbox("in", "inbox")])),
        ));
        state.take_work();
        let compose = compose_id(&state);
        type_in(&mut state, ComposeField::Subject, "Hi");
        state.draft_due(compose, 1);
        assert!(saves(&state.take_work()).is_empty());
        let line = state.draft_line().unwrap();
        assert!(line.contains(&crate::label("no-drafts-mailbox")), "{line}");
        // Autosave off: no timer at all.
        let mut state = composing();
        state.settings.compose.autosave = false;
        type_in(&mut state, ComposeField::Subject, "Hi");
        assert!(timers(&state.take_work()).is_empty());
        assert_eq!(state.draft_line(), None);
    }
}
