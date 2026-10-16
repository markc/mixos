// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Address completion in the compose dialog's To, Cc and Bcc fields.
//!
//! Only the entry being typed is completed: the one after the last `,` or
//! `;` outside quotes, by the same cutting rules a send parses the field
//! with (`jmap::split_addresses`). From two characters on, each edit arms a
//! [`SUGGEST_DELAY`] timer, and only the latest one asks the server
//! (`Contact/query` over every address book). Each request has a ticket, so
//! only the newest answer is shown.
//!
//! Contacts come first, one suggestion per address on the card. After them
//! come the account's recent correspondents: the To and Cc of the Sent pages
//! already loaded, and the From of the loaded inbox pages. That set is held
//! in memory only, costs no extra fetch and is never persisted. Addresses
//! already in the field are skipped, and at most [`MAX_SUGGESTIONS`] are
//! shown. Accepting one writes `Name <addr>, ` in place of the entry, with
//! the name quoted when it needs to be.

use std::ops::Range;

use jmap::{Contact, EmailAddress};
use serde_json::{Value, json};

use super::{ComposeField, ComposeState, State, Tag};
use crate::work::Work;

/// How long typing must pause before the server is asked.
pub const SUGGEST_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

/// The characters an entry needs before anything is suggested.
pub const MIN_CHARS: usize = 2;

/// The most suggestions shown.
pub const MAX_SUGGESTIONS: usize = 8;

/// The most recent correspondents kept per account.
const RECENT_CAP: usize = 500;

/// One completion.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub name: Option<String>,
    pub email: String,
    /// From a contact card (else a recent correspondent).
    pub contact: bool,
}

impl Suggestion {
    pub fn address(&self) -> EmailAddress {
        EmailAddress {
            name: self.name.clone(),
            email: self.email.clone(),
        }
    }
}

/// The dropdown under one address field.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggest {
    pub field: ComposeField,
    /// The entry being completed, as typed.
    pub query: String,
    /// The latest request: an answer with another ticket is dropped.
    pub ticket: u64,
    pub items: Vec<Suggestion>,
    /// The highlighted suggestion.
    pub cursor: usize,
}

/// A key the dropdown takes while its field has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestKey {
    Up,
    Down,
    /// Enter or Tab: take the highlighted suggestion.
    Accept,
    /// Esc: close the dropdown.
    Close,
}

/// The entry being typed: the last one's byte range, and its text trimmed,
/// an opening quote dropped.
pub fn last_entry(text: &str) -> (Range<usize>, String) {
    let range = jmap::split_addresses(text)
        .entries
        .pop()
        .unwrap_or(text.len()..text.len());
    let typed = text[range.clone()].trim();
    let typed = typed.strip_prefix('"').unwrap_or(typed).trim_start();
    (range, typed.to_owned())
}

/// `text` with the entry being typed replaced by `address` and a `, ` to
/// type the next one after.
pub fn accept(text: &str, address: &EmailAddress) -> String {
    let (range, _) = last_entry(text);
    let before = text[..range.start].trim_end();
    let mut out = before.to_owned();
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(&jmap::format_address(address));
    out.push_str(", ");
    out
}

/// The addresses already in the field, the one being typed apart,
/// lowercased.
pub fn present(text: &str) -> Vec<String> {
    let (range, _) = last_entry(text);
    jmap::draft_addresses(&text[..range.start])
        .into_iter()
        .map(|a| a.email.to_lowercase())
        .collect()
}

/// Where `query` matches inside `text`, ignoring case, as a byte range of
/// `text` (for the highlight).
pub fn matched(text: &str, query: &str) -> Option<Range<usize>> {
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return None;
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    (0..chars.len()).find_map(|start| {
        let mut at = start;
        let mut want = needle.iter();
        let mut pending = want.next();
        while let Some(&c) = pending {
            let (_, have) = *chars.get(at)?;
            let mut lower = have.to_lowercase();
            if lower.len() != 1 || lower.next() != Some(c) {
                return None;
            }
            at += 1;
            pending = want.next();
        }
        let end = chars.get(at).map_or(text.len(), |(i, _)| *i);
        Some(chars[start].0..end)
    })
}

/// How well `address` matches `query`: 0 for a prefix of the address or of
/// a word of the name, 1 for anything inside, `None` for no match.
fn rank(address: &EmailAddress, query: &str) -> Option<u8> {
    let name = address.name.as_deref().unwrap_or("");
    if matched(name, query).is_none() && matched(&address.email, query).is_none() {
        return None;
    }
    let leads = |text: &str| matched(text, query).is_some_and(|r| r.start == 0);
    let prefix = name.split([' ', '.', '-', '_']).any(leads) || leads(&address.email);
    Some(if prefix { 0 } else { 1 })
}

/// The suggestions for `query`: the contacts' addresses first, then the
/// recent ones, each group best match first and otherwise in the order
/// given; one per address, none already `present`, at most `limit`.
pub fn merge(
    query: &str,
    contacts: &[EmailAddress],
    recent: &[EmailAddress],
    present: &[String],
    limit: usize,
) -> Vec<Suggestion> {
    let mut seen: Vec<String> = present.to_vec();
    let mut out = Vec::new();
    for (group, contact) in [(contacts, true), (recent, false)] {
        let mut ranked: Vec<(u8, &EmailAddress)> = group
            .iter()
            .filter_map(|a| rank(a, query).map(|r| (r, a)))
            .collect();
        ranked.sort_by_key(|(r, _)| *r);
        for (_, address) in ranked {
            let key = address.email.to_lowercase();
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            out.push(Suggestion {
                name: address.name.clone().filter(|n| !n.trim().is_empty()),
                email: address.email.clone(),
                contact,
            });
        }
    }
    out.truncate(limit);
    out
}

/// One suggestion as the Bus shows it.
pub fn suggestion_json(item: &Suggestion) -> Value {
    json!({
        "name": item.name,
        "email": item.email,
        "address": jmap::format_address(&item.address()),
        "source": if item.contact { "contact" } else { "recent" },
    })
}

/// One address field's text.
fn field_text(compose: &ComposeState, field: ComposeField) -> Option<&str> {
    match field {
        ComposeField::To => Some(&compose.to_text),
        ComposeField::Cc => Some(&compose.cc_text),
        ComposeField::Bcc => Some(&compose.bcc_text),
        ComposeField::Subject | ComposeField::Body => None,
    }
}

impl State {
    /// Remember the correspondents of a page just loaded from `role`'s
    /// mailbox: whom Sent mail went to, whom inbox mail came from.
    pub(super) fn remember_recipients(&mut self, account: &str, emails: &[jmap::Email]) {
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return;
        };
        let role = view
            .mail
            .selected_mailbox()
            .and_then(|m| m.role.as_deref());
        let found: Vec<EmailAddress> = match role {
            Some("sent") => emails
                .iter()
                .flat_map(|e| e.to.iter().flatten().chain(e.cc.iter().flatten()))
                .cloned()
                .collect(),
            Some("inbox") => emails
                .iter()
                .flat_map(|e| e.from.iter().flatten())
                .cloned()
                .collect(),
            _ => return,
        };
        let me = view.entry.email.to_lowercase();
        for address in found {
            let key = address.email.to_lowercase();
            if key == me || !key.contains('@') {
                continue;
            }
            match view.recent.iter_mut().find(|a| a.email.to_lowercase() == key) {
                // A name learned later fills in a bare one.
                Some(known) => {
                    if known.name.is_none() {
                        known.name = address.name;
                    }
                }
                None => view.recent.push(address),
            }
        }
        view.recent.truncate(RECENT_CAP);
    }

    /// An address field changed: the entry being typed is looked up once
    /// typing pauses, or the dropdown closes when it is too short.
    pub(super) fn address_edited(&mut self, field: ComposeField) {
        let ticket = self.ticket();
        let Some(compose) = &mut self.compose else {
            return;
        };
        let Some(text) = field_text(compose, field) else {
            return;
        };
        let (_, query) = last_entry(text);
        if query.chars().count() < MIN_CHARS {
            compose.suggest = None;
            return;
        }
        let present = present(text);
        // What is shown already narrows at once; the server's answer follows.
        let items: Vec<Suggestion> = compose
            .suggest
            .take()
            .filter(|s| s.field == field)
            .map(|s| s.items)
            .unwrap_or_default()
            .into_iter()
            .filter(|item| rank(&item.address(), &query).is_some())
            .filter(|item| !present.contains(&item.email.to_lowercase()))
            .collect();
        compose.suggest = Some(Suggest {
            field,
            query,
            ticket,
            items,
            cursor: 0,
        });
        let compose = compose.id;
        self.queue(Work::SuggestTimer { compose, ticket });
    }

    /// Suggestion timer `ticket` ran out: the server is asked, unless a
    /// later edit has armed another.
    pub fn suggest_due(&mut self, compose: u64, ticket: u64) {
        let Some(open) = &self.compose else {
            return;
        };
        let Some(suggest) = open.suggest.as_ref().filter(|s| s.ticket == ticket) else {
            return;
        };
        if open.id != compose {
            return;
        }
        let (account, text) = (open.account.clone(), suggest.query.clone());
        let ready = self
            .account(&account)
            .is_some_and(|v| v.phase == super::Phase::Ready);
        if ready {
            self.queue(Work::Suggest {
                account,
                ticket,
                text,
                limit: MAX_SUGGESTIONS as u64,
            });
        } else {
            // No server to ask: the recent correspondents alone.
            self.suggested_with(ticket, &account, &[]);
        }
    }

    /// The server's contacts for request `ticket`: shown when it is still
    /// the latest, merged with the recent correspondents.
    pub(super) fn suggested(
        &mut self,
        tag: &Tag,
        ticket: u64,
        result: Result<Vec<Contact>, jmap::Error>,
    ) {
        let contacts: Vec<EmailAddress> = match result {
            Ok(cards) => cards.iter().flat_map(Contact::addresses).collect(),
            // The server could not answer: the recent ones still help.
            Err(_) => Vec::new(),
        };
        self.suggested_with(ticket, &tag.account, &contacts);
    }

    fn suggested_with(&mut self, ticket: u64, account: &str, contacts: &[EmailAddress]) {
        let recent = self
            .account(account)
            .map(|v| v.recent.clone())
            .unwrap_or_default();
        let Some(compose) = self.compose.as_mut().filter(|c| c.account == account) else {
            return;
        };
        let Some(field) = compose
            .suggest
            .as_ref()
            .filter(|s| s.ticket == ticket)
            .map(|s| s.field)
        else {
            return;
        };
        let present = field_text(compose, field).map(present).unwrap_or_default();
        if let Some(suggest) = &mut compose.suggest {
            suggest.items = merge(
                &suggest.query,
                contacts,
                &recent,
                &present,
                MAX_SUGGESTIONS,
            );
            suggest.cursor = 0;
        }
    }

    /// A key for the open dropdown.
    pub(super) fn suggest_key(&mut self, key: SuggestKey) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        let Some(suggest) = &mut compose.suggest else {
            return;
        };
        let count = suggest.items.len();
        match key {
            SuggestKey::Up if count > 0 => suggest.cursor = (suggest.cursor + count - 1) % count,
            SuggestKey::Down if count > 0 => suggest.cursor = (suggest.cursor + 1) % count,
            SuggestKey::Up | SuggestKey::Down => {}
            SuggestKey::Accept => {
                let at = suggest.cursor;
                self.suggest_accept(at);
            }
            SuggestKey::Close => compose.suggest = None,
        }
    }

    /// Take suggestion `index`: it replaces the entry being typed.
    pub(super) fn suggest_accept(&mut self, index: usize) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        let Some(suggest) = compose.suggest.take() else {
            return;
        };
        let Some(item) = suggest.items.get(index) else {
            return;
        };
        let text = match suggest.field {
            ComposeField::To => &mut compose.to_text,
            ComposeField::Cc => &mut compose.cc_text,
            ComposeField::Bcc => &mut compose.bcc_text,
            ComposeField::Subject | ComposeField::Body => return,
        };
        *text = accept(text, &item.address());
        self.note_edit();
    }

    /// `email.contacts.suggest`'s answer: the contacts the server found,
    /// merged with `account`'s recent correspondents.
    pub(super) fn suggest_answer(&self, account: &str, text: &str, limit: usize, contacts: &Value) -> Value {
        let cards: Vec<Contact> = contacts
            .get("contacts")
            .cloned()
            .and_then(|list| serde_json::from_value(list).ok())
            .unwrap_or_default();
        let found: Vec<EmailAddress> = cards.iter().flat_map(Contact::addresses).collect();
        let recent = self
            .account(account)
            .map(|v| v.recent.clone())
            .unwrap_or_default();
        let items = merge(text.trim(), &found, &recent, &[], limit);
        json!({ "suggestions": items.iter().map(suggestion_json).collect::<Vec<_>>() })
    }

    /// The open dropdown, as `email.info` shows it.
    pub(super) fn suggest_info(compose: &ComposeState) -> Value {
        match &compose.suggest {
            Some(s) => json!({
                "field": match s.field {
                    ComposeField::To => "to",
                    ComposeField::Cc => "cc",
                    ComposeField::Bcc => "bcc",
                    ComposeField::Subject | ComposeField::Body => "",
                },
                "query": s.query,
                "cursor": s.cursor,
                "suggestions": s.items.iter().map(suggestion_json).collect::<Vec<_>>(),
            }),
            None => Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str, email: &str) -> EmailAddress {
        EmailAddress {
            name: Some(name.into()),
            email: email.into(),
        }
    }

    #[test]
    fn the_entry_after_the_last_separator_is_replaced() {
        let ann = named("Ann Example", "ann@example.invalid");
        // Mid-list: what comes before stays as typed.
        assert_eq!(
            accept("bob@example.invalid; an", &ann),
            "bob@example.invalid; Ann Example <ann@example.invalid>, "
        );
        // A quoted name with a comma is one entry, not two.
        assert_eq!(
            accept("\"Lee, Cy\" <cy@example.invalid>, an", &ann),
            "\"Lee, Cy\" <cy@example.invalid>, Ann Example <ann@example.invalid>, "
        );
        // An entry being typed inside an open quote.
        assert_eq!(last_entry("bob@example.invalid, \"Lee, C").1, "Lee, C");
        // A trailing separator leaves nothing to complete.
        assert_eq!(last_entry("bob@example.invalid, ").1, "");
        // A name that needs quoting is quoted.
        assert_eq!(
            accept("le", &named("Lee, Cy", "cy@example.invalid")),
            "\"Lee, Cy\" <cy@example.invalid>, "
        );
    }

    #[test]
    fn contacts_rank_first_and_the_best_match_leads() {
        let contacts = [
            named("Joanna Anders", "jo@example.invalid"),
            named("Andy Pike", "andy@example.invalid"),
        ];
        let recent = [
            named("Ann Recent", "ann@example.invalid"),
            named("Dan", "dan@example.invalid"),
        ];
        let got: Vec<(String, bool)> = merge("an", &contacts, &recent, &[], 8)
            .into_iter()
            .map(|s| (s.email, s.contact))
            .collect();
        assert_eq!(
            got,
            [
                // A word of the name starts with it: first.
                ("jo@example.invalid".to_owned(), true),
                ("andy@example.invalid".to_owned(), true),
                ("ann@example.invalid".to_owned(), false),
                // Matched inside only: after.
                ("dan@example.invalid".to_owned(), false),
            ]
        );
    }

    #[test]
    fn one_suggestion_per_address_and_none_already_present() {
        let contacts = [named("Ann", "ann@example.invalid")];
        let recent = [
            named("Ann (old)", "ANN@example.invalid"),
            named("Anne", "anne@example.invalid"),
        ];
        let present = ["anne@example.invalid".to_owned()];
        let got = merge("ann", &contacts, &recent, &present, 8);
        assert_eq!(got.len(), 1);
        assert_eq!(
            (got[0].name.as_deref(), got[0].contact),
            (Some("Ann"), true)
        );
        let many: Vec<EmailAddress> = (0..20)
            .map(|i| named("Ann", &format!("ann{i}@example.invalid")))
            .collect();
        assert_eq!(merge("ann", &many, &[], &[], MAX_SUGGESTIONS).len(), 8);
        assert_eq!(
            present("Ann <ann@example.invalid>, bo"),
            ["ann@example.invalid"]
        );
    }

    fn card(name: &str, email: &str) -> Contact {
        serde_json::from_value(json!({
            "id": format!("P-{email}"),
            "addressBookId": "B1",
            "fullName": name,
            "email": email,
        }))
        .unwrap()
    }

    fn composing() -> State {
        let mut state = crate::state::tests::connected_many(&["a"]);
        state.apply_ui(crate::view::UiEvent::Compose);
        state.take_work();
        state
    }

    fn type_to(state: &mut State, text: &str) {
        state.apply_ui(crate::view::UiEvent::ComposeField(
            ComposeField::To,
            text.into(),
        ));
    }

    fn suggest_timers(work: &[Work]) -> Vec<u64> {
        work.iter()
            .filter_map(|w| match w {
                Work::SuggestTimer { ticket, .. } => Some(*ticket),
                _ => None,
            })
            .collect()
    }

    /// Only the latest edit's timer asks the server, and only the newest
    /// answer is shown: a late one is dropped.
    #[test]
    fn only_the_newest_answer_is_shown() {
        use crate::shell::{Done, Event};
        let mut state = composing();
        let compose = state.compose.as_ref().unwrap().id;
        type_to(&mut state, "a");
        assert!(state.compose.as_ref().unwrap().suggest.is_none(), "one character");
        type_to(&mut state, "an");
        type_to(&mut state, "ann");
        let timers = suggest_timers(&state.take_work());
        let [older, newest] = timers[..] else {
            panic!("{timers:?}");
        };
        state.suggest_due(compose, older);
        assert!(state.take_work().is_empty(), "a later edit voids it");
        state.suggest_due(compose, newest);
        match state.take_work().as_slice() {
            [Work::Suggest { ticket, text, limit: 8, .. }] => {
                assert_eq!((*ticket, text.as_str()), (newest, "ann"));
            }
            other => panic!("{other:?}"),
        }
        let tag = crate::state::tests::tag(&state, "a");
        let late = Ok(vec![card("Bob", "bob@example.invalid")]);
        state.apply(Event::Account(tag.clone(), Done::Suggested(older, late)));
        assert!(state.compose.as_ref().unwrap().suggest.as_ref().unwrap().items.is_empty());
        let found = Ok(vec![
            card("Ann Example", "ann@example.invalid"),
            card("Joanne Ann", "jo@example.invalid"),
        ]);
        state.apply(Event::Account(tag, Done::Suggested(newest, found)));
        let shown = state.compose.as_ref().unwrap().suggest.clone().unwrap();
        assert_eq!(shown.items.len(), 2);
        state.suggest_key(SuggestKey::Down);
        state.suggest_key(SuggestKey::Accept);
        let compose = state.compose.as_ref().unwrap();
        assert_eq!(compose.to_text, "Joanne Ann <jo@example.invalid>, ");
        assert!(compose.suggest.is_none());
        assert!(compose.save.edits > 3, "an accept is an edit");
    }

    /// The recent set: the To and Cc of loaded Sent pages and the From of
    /// loaded inbox pages, never the account itself, and nothing else.
    #[test]
    fn recent_correspondents_come_from_loaded_sent_and_inbox_pages() {
        use crate::shell::{Done, Event};
        let mut state = crate::state::tests::connected_many(&["a"]);
        let tag = crate::state::tests::tag(&state, "a");
        let boxes = ["in:inbox", "st:sent", "x:archive"].map(|spec| {
            let (id, role) = spec.split_once(':').unwrap();
            jmap::Mailbox {
                id: id.into(),
                role: Some(role.into()),
                ..jmap::Mailbox::default()
            }
        });
        state.apply(Event::Account(tag.clone(), Done::Mailboxes(Ok(boxes.to_vec()))));
        let page = |emails: Value| {
            Ok(jmap::EmailPage {
                emails: serde_json::from_value(emails).unwrap(),
                total: 1,
            })
        };
        let ticket = |state: &State| state.account("a").unwrap().tickets.page;
        let t = ticket(&state);
        state.apply(Event::Account(tag.clone(), Done::Page(t, page(json!([{
            "id": "E1",
            "from": [{ "name": "Cy", "email": "cy@example.invalid" }],
            "to": [{ "email": "ann@a.example.invalid" }]
        }])))));
        state.select_mailbox("a", "st".into());
        let t = ticket(&state);
        state.apply(Event::Account(tag.clone(), Done::Page(t, page(json!([{
            "id": "E2",
            "from": [{ "email": "ann@a.example.invalid" }],
            "to": [{ "name": "Dee", "email": "dee@example.invalid" }],
            "cc": [{ "email": "eve@example.invalid" }]
        }])))));
        state.select_mailbox("a", "x".into());
        let t = ticket(&state);
        state.apply(Event::Account(tag, Done::Page(t, page(json!([{
            "id": "E3",
            "from": [{ "email": "zed@example.invalid" }]
        }])))));
        let recent: Vec<&str> = state.account("a").unwrap().recent.iter().map(|a| a.email.as_str()).collect();
        assert_eq!(recent, ["cy@example.invalid", "dee@example.invalid", "eve@example.invalid"]);
    }

    #[test]
    fn the_match_is_found_ignoring_case() {
        assert_eq!(matched("Ann Example", "EX"), Some(4..6));
        assert_eq!(matched("Ann", "x"), None);
        assert_eq!(matched("Zoë Ø", "ø"), Some(5..7));
    }
}
