// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The state's side of printing: the page is built here (pure, from the
//! email and how the reader shows it) and the shell writes and opens it.

use jmap::Email;
use serde_json::json;

use super::{State, code};
use crate::work::Work;

impl State {
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
        open(&mut state, email("E1", "Plan", "# Heading\n\n- one\n- two\n"));
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
