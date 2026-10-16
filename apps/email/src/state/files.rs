// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The state's side of printing and of attachments. A print page is built
//! here (pure, from the email and how the reader shows it) and the shell
//! writes and opens it. An attachment's save or open is decided here (which
//! part, whether it may be opened) and the shell downloads it on its
//! effects runtime, writes it, and opens it.

use jmap::Email;
use serde_json::json;

use super::{Dialog, State, code};
use crate::attachments::{self, Target};
use crate::work::Work;

impl State {
    /// `mail.attachments`: show the open email's attachments.
    pub fn open_attachments(&mut self) {
        if self
            .reading()
            .is_some_and(|(_, e)| !attachments::of(&e).is_empty())
        {
            self.ui.dialog = Some(Dialog::Attachments);
        }
    }

    /// The popup's Save or Open (`open`) of the open email's `part`.
    pub fn attachment(&mut self, part: &str, open: bool) {
        let Some((account, email)) = self.reading() else {
            return;
        };
        let target = if open {
            Target::Open
        } else {
            Target::Save(None)
        };
        if let Err((_, message)) = self.fetch_attachment(&account, &email, part, target, None) {
            self.status = message;
        }
    }

    /// The popup's Save All: every attachment that can be downloaded.
    pub fn save_all_attachments(&mut self) {
        let Some((account, email)) = self.reading() else {
            return;
        };
        let parts: Vec<String> = attachments::of(&email)
            .iter()
            .filter(|a| a.downloadable())
            .filter_map(|a| a.part_id.clone())
            .filter(|part| !self.attaching.values().any(|p| p == part))
            .collect();
        for part in parts {
            let _ = self.fetch_attachment(&account, &email, &part, Target::Save(None), None);
        }
    }

    /// Download `email`'s `part` and put it where `target` says; `reply`
    /// (a Bus request) hears the path. Refused, with the reason, when the
    /// part is not there, cannot be decoded, or is a program to open; a
    /// refusal is answered to `reply` here too.
    pub fn fetch_attachment(
        &mut self,
        account: &str,
        email: &Email,
        part: &str,
        target: Target,
        reply: Option<u64>,
    ) -> Result<(), (&'static str, String)> {
        let list = attachments::of(email);
        let found = list
            .iter()
            .enumerate()
            .find(|(_, a)| a.part_id.as_deref() == Some(part));
        let refused = match found {
            None => Some((
                code::NOT_FOUND,
                crate::label_with("no-such-attachment", &[("part", part)]),
            )),
            Some((_, a)) if !a.downloadable() => Some((
                code::INVALID_ARGUMENT,
                crate::label("attachment-undecodable"),
            )),
            Some((_, a))
                if target == Target::Open
                    && attachments::refuses_open(&a.mime(), a.name.as_deref()) =>
            {
                Some((code::REFUSED, crate::label("attachment-save-only")))
            }
            _ => None,
        };
        if let Some((code, message)) = refused {
            if let Some(reply) = reply {
                self.refuse(reply, code, &message);
            }
            return Err((code, message));
        }
        let Some((index, attachment)) = found else {
            return Ok(());
        };
        let ticket = self.ticket();
        self.attaching.insert(ticket, part.to_owned());
        if let Some(reply) = reply {
            // A download may land on disk whatever becomes of the window.
            self.outstanding.insert(reply, true);
        }
        self.status = crate::label("attachment-downloading");
        self.queue(Work::Attachment {
            account: account.to_owned(),
            ticket,
            blob: attachment.blob_id.clone().unwrap_or_default(),
            name: attachment.name.clone(),
            index: index + 1,
            mime: attachment.mime(),
            target,
            reply,
        });
        Ok(())
    }

    /// An attachment download finished: saved (or opened) at the path, or
    /// why not, on the status line and to its caller.
    pub fn attached(
        &mut self,
        ticket: u64,
        reply: Option<u64>,
        open: bool,
        result: Result<String, String>,
    ) {
        self.attaching.remove(&ticket);
        match result {
            Ok(path) => {
                let key = if open {
                    "attachment-opened"
                } else {
                    "attachment-saved"
                };
                self.status = crate::label_with(key, &[("path", &path)]);
                if let Some(reply) = reply {
                    self.reply(reply, 0, json!({ "path": path }));
                }
            }
            Err(error) => {
                if let Some(reply) = reply {
                    self.refuse(reply, code::FILE, &error);
                }
                self.status = error;
            }
        }
    }

    /// `mail.print`: print the open email of the current account.
    pub fn print_open(&mut self) {
        if let Some((account, email)) = self.reading() {
            self.print_email(&account, &email, None);
        }
    }

    /// Print `account`'s `email`: its body as the reader shows it (the
    /// person's Markdown or source choice when it is the open email), at
    /// the reader's text size. `reply` (a Bus request) hears the page's
    /// path once the browser has it.
    pub fn print_email(&mut self, account: &str, email: &Email, reply: Option<u64>) {
        let format = self
            .account(account)
            .map(|v| &v.mail.reader)
            .filter(|reader| reader.is_for(email))
            .map_or_else(|| crate::reader::detect(email), |reader| reader.format());
        let html = crate::print::page(email, format, self.settings.reader.font_size);
        if let Some(reply) = reply {
            // The page may be written and shown whatever becomes of the
            // window, so a close leaves its outcome unknown.
            self.outstanding.insert(reply, true);
        }
        self.status = crate::label("printing");
        self.queue(Work::Print { html, reply });
    }

    /// A print finished: the browser has the page, or the status line names
    /// why not (and where the page is, when it was written).
    pub fn printed(&mut self, reply: Option<u64>, result: Result<String, String>) {
        match result {
            Ok(path) => {
                self.status = crate::label_with("printed", &[("path", &path)]);
                if let Some(reply) = reply {
                    self.reply(reply, 0, json!({ "path": path }));
                }
            }
            Err(error) => {
                if let Some(reply) = reply {
                    self.refuse(reply, code::FILE, &error);
                }
                self.status = error;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::attachments::Target;
    use crate::state::tests::{connected_many, tag};
    use crate::state::{State, code};
    use crate::verbs::{Query, Then};
    use crate::work::Work;

    fn email(id: &str, subject: &str, body: &str) -> jmap::Email {
        serde_json::from_value(json!({
            "id": id,
            "subject": subject,
            "from": [{ "name": "Bob", "email": "bob@example.invalid" }],
            "textBody": [{ "partId": "1", "type": "text/markdown" }],
            "bodyValues": { "1": { "value": body } },
        }))
        .unwrap()
    }

    fn open(state: &mut State, email: jmap::Email) {
        let view = state.accounts.iter_mut().find(|v| v.id() == "a").unwrap();
        view.mail.reader.open(&email);
        view.mail.open = Some(email);
    }

    fn bus(state: &mut State, verb: &str, args: Value) {
        crate::verbs::command(
            state,
            &crate::commands::registry(),
            &crate::strings(),
            1,
            verb,
            &args.to_string(),
        );
    }

    fn printed(work: &[Work]) -> Vec<(&str, Option<u64>)> {
        work.iter()
            .filter_map(|w| match w {
                Work::Print { html, reply } => Some((html.as_str(), *reply)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn print_builds_the_page_as_the_reader_shows_it() {
        let mut state = connected_many(&["a"]);
        open(
            &mut state,
            email("E1", "Plan", "# Heading\n\n- one\n- two\n"),
        );
        state.print_open();
        let work = state.take_work();
        let [(html, None)] = printed(&work)[..] else {
            panic!("{work:?}");
        };
        assert!(html.contains("<h1>Heading</h1>"), "rendered as Markdown");
        // Flipped to its source, it prints as the source.
        state.toggle_reader();
        state.print_open();
        let work = state.take_work();
        let [(html, None)] = printed(&work)[..] else {
            panic!("{work:?}");
        };
        assert!(html.contains("<pre class=\"plain\"># Heading"));
        assert_eq!(state.status, crate::label("printing"));
    }

    #[test]
    fn the_print_verb_prints_the_open_email_or_fetches_another() {
        let mut state = connected_many(&["a"]);
        bus(&mut state, "email.print", json!({}));
        let replies = state.take_replies();
        assert_eq!(replies[0].body["error_code"], code::NOT_FOUND);

        open(&mut state, email("E1", "Plan", "hi"));
        bus(&mut state, "email.print", json!({ "id": "E1" }));
        let work = state.take_work();
        assert!(matches!(printed(&work)[..], [(_, Some(1))]), "{work:?}");

        bus(&mut state, "email.print", json!({ "id": "E9" }));
        let work = state.take_work();
        assert!(matches!(
            work.as_slice(),
            [Work::Query { reply: 1, query: Query::Get { id }, then: Then::Print, .. }] if id == "E9"
        ));
        let fetched = serde_json::to_value(email("E9", "Other", "text")).unwrap();
        let tag = tag(&state, "a");
        state.answered(&tag, 1, Then::Print, Ok(fetched));
        let work = state.take_work();
        let [(html, Some(1))] = printed(&work)[..] else {
            panic!("{work:?}");
        };
        assert!(html.contains("<title>Other</title>"));
        assert!(state.take_replies().is_empty(), "answered once printed");
    }

    fn with_attachments() -> jmap::Email {
        serde_json::from_value(json!({
            "id": "E1",
            "attachments": [
                { "partId": "2", "blobId": "B2", "size": 9, "name": "a.pdf", "type": "application/pdf" },
                { "partId": "3", "blobId": "B3", "size": 9, "name": "go.sh", "type": "application/x-shellscript" },
                { "partId": "4", "size": 9, "type": "image/png", "undecodable": true }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn a_download_names_its_part_and_refuses_what_it_must() {
        let mut state = connected_many(&["a"]);
        let email = with_attachments();
        let ok = state.fetch_attachment("a", &email, "2", Target::Save(None), None);
        assert!(ok.is_ok());
        match state.take_work().as_slice() {
            [
                Work::Attachment {
                    account,
                    blob,
                    name,
                    index: 1,
                    mime,
                    target: Target::Save(None),
                    reply: None,
                    ..
                },
            ] => {
                assert_eq!((account.as_str(), blob.as_str()), ("a", "B2"));
                assert_eq!(
                    (name.as_deref(), mime.as_str()),
                    (Some("a.pdf"), "application/pdf")
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(state.attaching.values().collect::<Vec<_>>(), ["2"]);
        // A script saves but never opens; an undecodable part neither.
        assert!(
            state
                .fetch_attachment("a", &email, "3", Target::Save(None), None)
                .is_ok()
        );
        let refused = state.fetch_attachment("a", &email, "3", Target::Open, Some(7));
        assert_eq!(refused.unwrap_err().0, code::REFUSED);
        let refused = state.fetch_attachment("a", &email, "4", Target::Save(None), None);
        assert_eq!(refused.unwrap_err().0, code::INVALID_ARGUMENT);
        let refused = state.fetch_attachment("a", &email, "9", Target::Save(None), None);
        assert_eq!(refused.unwrap_err().0, code::NOT_FOUND);
        let replies = state.take_replies();
        assert_eq!(replies.len(), 1, "the Bus caller heard its refusal");
        assert_eq!(replies[0].body["error_code"], code::REFUSED);
    }

    #[test]
    fn the_attachment_verbs_validate_then_fetch() {
        let mut state = connected_many(&["a"]);
        bus(&mut state, "email.attachments", json!({ "id": "E1" }));
        assert!(matches!(
            state.take_work().as_slice(),
            [Work::Query { query: Query::Attachments { id }, then: Then::Answer, .. }] if id == "E1"
        ));
        for dir in ["relative/dir", "/nonexistent/email-attach-test"] {
            bus(
                &mut state,
                "email.attachment.save",
                json!({ "id": "E1", "part": "2", "dir": dir }),
            );
            let replies = state.take_replies();
            assert_eq!(
                replies[0].body["error_code"],
                code::INVALID_ARGUMENT,
                "{dir}"
            );
        }
        let dir = std::env::temp_dir();
        bus(
            &mut state,
            "email.attachment.save",
            json!({ "id": "E1", "part": "2", "dir": dir }),
        );
        let work = state.take_work();
        let [
            Work::Query {
                query: Query::Get { .. },
                then,
                ..
            },
        ] = work.as_slice()
        else {
            panic!("{work:?}");
        };
        assert_eq!(
            then,
            &Then::Save {
                part: "2".into(),
                dir: Some(dir.clone())
            }
        );
        let tag = tag(&state, "a");
        let fetched = serde_json::to_value(with_attachments()).unwrap();
        state.answered(&tag, 1, then.clone(), Ok(fetched));
        assert!(matches!(
            state.take_work().as_slice(),
            [Work::Attachment { target: Target::Save(Some(d)), reply: Some(1), .. }] if *d == dir
        ));
        // Closing with it under way: the caller hears, nothing waits.
        state.close_out();
        assert!(state.attaching.is_empty());
        let replies = state.take_replies();
        assert_eq!(replies[0].body["error_code"], code::OUTCOME_UNKNOWN);
    }

    #[test]
    fn a_print_answers_its_path_or_why_not() {
        let mut state = connected_many(&["a"]);
        open(&mut state, email("E1", "Plan", "hi"));
        bus(&mut state, "email.print", json!({}));
        state.take_work();
        state.printed(Some(1), Ok("/run/user/1/email-print/p.html".into()));
        let replies = state.take_replies();
        assert_eq!(
            (replies[0].rc, &replies[0].body),
            (0, &json!({ "path": "/run/user/1/email-print/p.html" }))
        );
        assert!(state.status.contains("/run/user/1/email-print/p.html"));
        state.printed(Some(2), Err("no browser; it is at /x.html".into()));
        let replies = state.take_replies();
        assert_eq!(replies[0].body["error_code"], code::FILE);
        assert_eq!(state.status, "no browser; it is at /x.html");
    }
}
