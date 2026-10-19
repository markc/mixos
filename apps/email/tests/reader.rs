// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The reader's Markdown rendering, driven through kittest with no
//! network: a Markdown-written text body renders (and flips to its source
//! and back through `view.reader.markdown`, which `email.info` reports), a
//! plain one stays plain, and offscreen snapshots of the reader panel in a
//! dark and a light theme. Regenerate the images with `UPDATE_SNAPSHOTS=1`
//! and look at them before committing.
use design::{DesignContext, Mode, Scheme};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use email::secrets::Secrets;
use email::shell::{Done, Event};
use email::state::{Mode as Surface, SessionInfo, State, Tag, UiState};
use email::view::{UiEvent, view};
use email::work::Work;
use email::{commands, label, strings};
use jmap::EmailPage;
use settings::email::{AccountEntry, Email as EmailSettings};
use toolkit::Theme;
use toolkit::command::Origin;

const MARKDOWN: &str = "\
Hi Ann,

Here is where the **build cluster** move stands.

## What changed

- Rust builds run on the cluster, never locally
- Snapshots are fetched and *looked at* before a commit
  - fixtures stay small
- [x] lockfile committed
- [ ] docs regenerated

## Running it

```sh
mix cbc-build.mix --worker build-b -- cargo test -p email
```

| worker  | load | jobs |
|---------|-----:|-----:|
| build-a | 0.4  | 2    |
| build-b | 1.2  | 5    |

> The lock serialises jobs per checkout, so a second caller waits.

Details are in [the build guide](https://mixos.dev/docs/dev/building/). Run `mix --version` to check.

Bob
";

const PLAIN: &str = "\
Hi Ann,

Lunch on Thursday works. I'll book the usual place for 12:30
and let the others know.

- Bob

On Tue, 14 Oct 2026 at 09:12, Ann <ann@example.invalid> wrote:
> Are we still on for lunch this week?
> - the room is free on Thursday
";

fn tag(state: &State, account: &str) -> Tag {
    Tag {
        account: account.into(),
        generation: state.account(account).unwrap().generation(),
    }
}

fn done(state: &mut State, account: &str, done: Done) {
    let tag = tag(state, account);
    state.apply(Event::Account(tag, done));
}

/// One connected account with its inbox, and `body` (of part type `kind`)
/// open in the reader the way an open really lands.
fn opened(kind: &str, body: &str) -> State {
    // Dates (the list's, a reply's attribution) render in UTC, whatever
    // the worker's zone.
    // SAFETY: every test sets the same value before any date is drawn.
    unsafe { std::env::set_var("TZ", "UTC") };
    let entry = AccountEntry {
        id: "home".into(),
        name: "Home".into(),
        url: "https://home.example.invalid".into(),
        email: "ann@home.example.invalid".into(),
        ca_path: None,
        enabled: true,
        signature: String::new(),
        identities: Vec::new(),
    };
    let settings = EmailSettings {
        accounts: vec![entry],
        ..EmailSettings::default()
    };
    let mut secrets = Secrets::default();
    secrets.set("home", "secret");
    let mut state = State::new(
        UiState::default(),
        Surface::Mail,
        None,
        settings.clone(),
        secrets,
    );
    state.set_settings(settings);
    let home = tag(&state, "home");
    state.apply(Event::Connected(
        home,
        Ok(SessionInfo {
            account_name: "Account 7".into(),
            account_id: "7".into(),
        }),
    ));
    done(
        &mut state,
        "home",
        Done::Mailboxes(Ok(vec![jmap::Mailbox {
            id: "m-in".into(),
            name: "Inbox".into(),
            role: Some("inbox".into()),
            ..jmap::Mailbox::default()
        }])),
    );
    let header = serde_json::json!({
        "id": "E1",
        "subject": "Build cluster: where things stand",
        "from": [{ "name": "Bob", "email": "bob@example.invalid" }],
        "to": [{ "name": "Ann", "email": "ann@home.example.invalid" }],
        "receivedAt": "2026-10-13T01:02:03Z",
        "keywords": { "$seen": true }
    });
    done(
        &mut state,
        "home",
        Done::Page(
            1,
            Ok(EmailPage {
                emails: vec![serde_json::from_value(header.clone()).unwrap()],
                total: 1,
            }),
        ),
    );
    state.take_work();
    state.apply_ui(UiEvent::OpenEmail("home".into(), "E1".into()));
    let ticket = state
        .take_work()
        .iter()
        .find_map(|w| match w {
            Work::OpenEmail { ticket, .. } => Some(*ticket),
            _ => None,
        })
        .expect("an open is issued");
    let mut full = header;
    full["textBody"] = serde_json::json!([{ "partId": "1", "type": kind }]);
    full["bodyValues"] = serde_json::json!({ "1": { "value": body } });
    done(
        &mut state,
        "home",
        Done::Opened(
            ticket,
            None,
            Box::new(Ok(serde_json::from_value(full).unwrap())),
        ),
    );
    state
}

fn theme(scheme: Scheme, mode: Mode) -> Theme {
    Theme::for_context(DesignContext {
        scheme,
        mode,
        ..DesignContext::default()
    })
}

/// The reader panel alone, as the window frames it, over `state`; it
/// keeps the events it reported.
fn harness(state: State, theme: &Theme) -> Harness<'static, (State, Vec<UiEvent>)> {
    // SAFETY: every test sets the same value before any date is drawn.
    unsafe { std::env::set_var("TZ", "UTC") };
    let registry = commands::registry();
    let strings = strings();
    let mut h = Harness::builder()
        .with_size(egui::vec2(620.0, 980.0))
        .build_ui_state(
            move |ui, (state, seen): &mut (State, Vec<UiEvent>)| {
                let mut events = Vec::new();
                egui::CentralPanel::default()
                    .frame(toolkit::bars::dock_frame(ui.ctx()))
                    .show(ui, |ui| {
                        let title = label("reader");
                        toolkit::panel::group(ui, "reader", &[title.as_str()], |ui, _| {
                            email::view::mail::reader(
                                ui,
                                state,
                                state.current().unwrap(),
                                &registry,
                                &strings,
                                2.0,
                                &mut events,
                            );
                        });
                    });
                seen.extend(events);
            },
            (state, Vec::new()),
        );
    toolkit::install(&h.ctx, theme);
    h.run();
    h
}

/// The whole window over `state`, running the commands its events name as
/// the shell does.
fn window(state: State, theme: &Theme) -> Harness<'static, (State, Vec<UiEvent>)> {
    // SAFETY: every test sets the same value before any date is drawn.
    unsafe { std::env::set_var("TZ", "UTC") };
    let registry = commands::registry();
    let strings = strings();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1180.0, 760.0))
        .build_ui_state(
            move |ui, (state, seen): &mut (State, Vec<UiEvent>)| {
                let events = view(ui, state, &registry, &strings, 2.0);
                for event in events {
                    seen.push(event.clone());
                    if let UiEvent::Command(id, origin) = event {
                        let _ = registry.execute(id, state, origin);
                    } else {
                        state.apply_ui(event);
                    }
                }
            },
            (state, Vec::new()),
        );
    toolkit::install(&h.ctx, theme);
    // Friday 2026-10-16 09:30 UTC: the list's dates do not move with the
    // clock.
    email::view::mail::pin_clock(&h.ctx, 1_792_143_000, 0);
    h.run();
    h
}

fn reader_info(state: &State) -> serde_json::Value {
    state.info()["accounts"][0]["mail"]["reader"].clone()
}

#[test]
fn a_markdown_written_body_renders_as_markdown() {
    let h = harness(
        opened("text/plain", MARKDOWN),
        &theme(Scheme::Pro, Mode::Light),
    );
    for text in [
        "What changed",
        "Running it",
        "worker",
        "build-b",
        "fixtures stay small",
    ] {
        assert!(h.query_by_label(text).is_some(), "{text}");
    }
    // No raw syntax is on screen.
    assert!(h.query_by_label_contains("## What changed").is_none());
    assert_eq!(
        reader_info(&h.state().0),
        serde_json::json!({ "format": "markdown", "auto": true })
    );
    assert_eq!(
        commands::registry().checked("view.reader.markdown", &h.state().0),
        Some(true),
        "View › Message › Render Markdown is ticked"
    );
}

#[test]
fn the_toggle_shows_the_source_and_back() {
    let mut h = harness(
        opened("text/plain", MARKDOWN),
        &theme(Scheme::Pro, Mode::Light),
    );
    let registry = commands::registry();
    // The menu item and `email.execute` run the same command.
    registry
        .execute("view.reader.markdown", &mut h.state_mut().0, Origin::Bus)
        .unwrap();
    h.run();
    assert_eq!(
        reader_info(&h.state().0),
        serde_json::json!({ "format": "plain", "auto": false })
    );
    assert!(h.query_by_label(MARKDOWN).is_some(), "the source, verbatim");
    assert_eq!(
        registry.checked("view.reader.markdown", &h.state().0),
        Some(false)
    );
    registry
        .execute("view.reader.markdown", &mut h.state_mut().0, Origin::Bus)
        .unwrap();
    h.run();
    assert_eq!(
        reader_info(&h.state().0),
        serde_json::json!({ "format": "markdown", "auto": true })
    );
}

#[test]
fn a_text_markdown_part_renders_whatever_it_holds() {
    let h = harness(
        opened("text/markdown", "Plain words *only*."),
        &theme(Scheme::Pro, Mode::Light),
    );
    assert_eq!(reader_info(&h.state().0)["format"], "markdown");
    assert!(h.query_by_label("Plain words only.").is_some());
}

#[test]
fn a_plain_body_stays_plain() {
    let h = harness(
        opened("text/plain", PLAIN),
        &theme(Scheme::Pro, Mode::Light),
    );
    assert!(h.query_by_label(PLAIN).is_some(), "one label, verbatim");
    assert_eq!(
        reader_info(&h.state().0),
        serde_json::json!({ "format": "plain", "auto": true })
    );
}

#[test]
fn an_html_text_body_is_never_markdown() {
    let state = opened("text/html", "<h1>Hi</h1>\n\n# not markdown\n\n## either\n");
    assert_eq!(reader_info(&state)["format"], "plain");
    let registry = commands::registry();
    assert!(
        registry
            .execute("view.reader.markdown", &mut { state }, Origin::Bus)
            .is_err()
    );
}

#[test]
fn the_toolbar_buttons_are_their_commands() {
    let mut h = harness(
        opened("text/plain", PLAIN),
        &theme(Scheme::Pro, Mode::Light),
    );
    for (text, id) in [
        (label("reply"), "mail.reply"),
        (label("reply-all"), "mail.reply_all"),
        (label("forward"), "mail.forward"),
        (label("delete"), "mail.delete"),
    ] {
        h.state_mut().1.clear();
        h.get_by_label(&text).click();
        h.run();
        assert_eq!(h.state().1, [UiEvent::Command(id, Origin::Ui)], "{text}");
    }
    // The split part offers the quoting choices.
    h.get_by_label(&label("reply-options")).click();
    h.run();
    h.state_mut().1.clear();
    h.get_by_label(&label("reply-unquoted")).click();
    h.run();
    assert_eq!(
        h.state().1,
        [UiEvent::Command("mail.reply_unquoted", Origin::Ui)]
    );
}

/// Reply, Reply All and Forward need an open email; Delete needs a target
/// in the list (phase 3b: it acts on the selection, not the open email).
#[test]
fn with_no_email_open_the_toolbar_is_disabled() {
    use egui_kittest::kittest::NodeT;
    let closed = || {
        let mut state = opened("text/plain", PLAIN);
        state.accounts[0].mail.open = None;
        state
    };
    let h = harness(closed(), &theme(Scheme::Pro, Mode::Light));
    for key in ["reply", "reply-all", "forward"] {
        let text = label(key);
        let node = h.get_by_label(&text);
        assert!(node.accesskit_node().is_disabled(), "{key}");
    }
    let delete = label("delete");
    assert!(
        !h.get_by_label(&delete).accesskit_node().is_disabled(),
        "E1 is still selected in the list"
    );
    let mut state = closed();
    state.accounts[0].mail.table.selection.clear();
    state.accounts[0].mail.table.focus = None;
    let h = harness(state, &theme(Scheme::Pro, Mode::Light));
    assert!(h.get_by_label(&delete).accesskit_node().is_disabled());
}

#[test]
fn a_quoted_reply_quotes_and_an_unquoted_one_does_not() {
    let registry = commands::registry();
    let mut state = opened("text/plain", PLAIN);
    registry
        .execute("mail.reply_quoted", &mut state, Origin::Bus)
        .unwrap();
    let body = state.compose.take().unwrap().draft.body;
    assert!(body.contains("Bob wrote:"), "{body}");
    assert!(body.contains("\n> Lunch on Thursday works."), "{body}");
    assert!(
        body.contains("\n>> Are we still on for lunch this week?"),
        "{body}"
    );
    registry
        .execute("mail.reply_unquoted", &mut state, Origin::Bus)
        .unwrap();
    let body = state.compose.take().unwrap().draft.body;
    assert!(!body.contains("wrote:"), "{body}");
}

#[test]
fn mark_unread_keeps_the_email_open_and_unread() {
    let registry = commands::registry();
    let mut state = opened("text/plain", PLAIN);
    let strings = strings();
    assert_eq!(
        registry
            .describe(&state, &strings)
            .iter()
            .find(|c| c.id == "mail.mark_unread")
            .unwrap()
            .label,
        label("mark-unread")
    );
    registry
        .execute("mail.mark_unread", &mut state, Origin::Bus)
        .unwrap();
    let open = state.current().unwrap().mail.open.clone().unwrap();
    assert!(!open.is_read(), "marked unread at once");
    assert!(
        !state.current().unwrap().mail.emails[0].is_read(),
        "the row too"
    );
    assert_eq!(
        registry
            .describe(&state, &strings)
            .iter()
            .find(|c| c.id == "mail.mark_unread")
            .unwrap()
            .label,
        label("mark-read")
    );
    state.take_work();
    // A refresh of the same mailbox redraws it; it is never re-marked read.
    state.refresh();
    assert!(
        !state
            .take_work()
            .iter()
            .any(|w| matches!(w, Work::MarkRead { read: true, .. })),
        "only a fresh open marks read"
    );
    assert!(
        !state
            .current()
            .unwrap()
            .mail
            .open
            .as_ref()
            .unwrap()
            .is_read()
    );
    // M again marks it read.
    registry
        .execute("mail.mark_unread", &mut state, Origin::Bus)
        .unwrap();
    assert!(
        state
            .current()
            .unwrap()
            .mail
            .open
            .as_ref()
            .unwrap()
            .is_read()
    );
}

#[test]
fn star_toggles_and_relabels() {
    let registry = commands::registry();
    let strings = strings();
    let mut state = opened("text/plain", PLAIN);
    let label_now = |state: &State| {
        registry
            .describe(state, &strings)
            .into_iter()
            .find(|c| c.id == "mail.flag")
            .unwrap()
            .label
    };
    assert_eq!(label_now(&state), label("flag"));
    registry
        .execute("mail.flag", &mut state, Origin::Bus)
        .unwrap();
    assert_eq!(label_now(&state), label("unflag"));
    registry
        .execute("mail.flag", &mut state, Origin::Bus)
        .unwrap();
    assert_eq!(label_now(&state), label("flag"));
}

#[test]
fn bare_key_and_delete_shortcuts_are_browsing_only() {
    use toolkit::command::Scope;
    let registry = commands::registry();
    for id in [
        "mail.flag",
        "mail.mark_unread",
        "mail.junk",
        "mail.not_junk",
        "mail.delete",
    ] {
        assert_eq!(registry.scope_of(id), Scope::Browsing, "{id}");
    }
    assert_eq!(registry.scope_of("mail.reply"), Scope::Anywhere);
}

/// The batched action the state queued last: its ids and change.
fn last_batch(state: &mut State) -> Option<(Vec<String>, jmap::EmailChange)> {
    state.take_work().into_iter().rev().find_map(|w| match w {
        Work::Batch { batch, .. } => Some((batch.ids, batch.change)),
        _ => None,
    })
}

#[test]
fn junk_moves_to_the_junk_mailbox_and_not_junk_back_to_the_inbox() {
    let registry = commands::registry();
    let strings = strings();
    let mut state = opened("text/plain", PLAIN);
    // No Junk mailbox yet: disabled, and the label says why.
    let junk = |state: &State| {
        registry
            .describe(state, &strings)
            .into_iter()
            .find(|c| c.id == "mail.junk")
            .unwrap()
    };
    assert!(!junk(&state).enabled);
    assert_eq!(junk(&state).label, label("junk-none"));
    state.accounts[0].mail.mailboxes.push(jmap::Mailbox {
        id: "m-junk".into(),
        name: "Junk".into(),
        role: Some("junk".into()),
        ..jmap::Mailbox::default()
    });
    assert!(junk(&state).enabled);
    assert_eq!(junk(&state).label, label("junk"));
    assert!(
        registry
            .execute("mail.not_junk", &mut state, Origin::Bus)
            .is_err(),
        "not in Junk"
    );
    state.take_work();
    registry
        .execute("mail.junk", &mut state, Origin::Bus)
        .unwrap();
    assert!(matches!(
        last_batch(&mut state),
        Some((ids, jmap::EmailChange::Move { mailbox })) if ids == ["E1"] && mailbox == "m-junk"
    ));
    // Reading the Junk mailbox: J is off, Shift+J moves back to the inbox.
    state.accounts[0].mail.selected = Some("m-junk".into());
    assert!(
        registry
            .execute("mail.junk", &mut state, Origin::Bus)
            .is_err()
    );
    registry
        .execute("mail.not_junk", &mut state, Origin::Bus)
        .unwrap();
    assert!(matches!(
        last_batch(&mut state),
        Some((_, jmap::EmailChange::Move { mailbox })) if mailbox == "m-in"
    ));
}

/// `state`s open email with three attachments: a PDF, a script, and a
/// part maild could not decode.
fn with_attachments(mut state: State) -> State {
    let list: Vec<jmap::Attachment> = serde_json::from_value(serde_json::json!([
        { "partId": "2", "blobId": "B2", "size": 248_000, "name": "Quarterly build cluster capacity plan (final, revised).pdf", "type": "application/pdf" },
        { "partId": "3", "blobId": "B3", "size": 912, "name": "../../install.sh", "type": "application/x-shellscript" },
        { "partId": "4", "size": 40, "name": "winmail.dat", "type": "application/ms-tnef", "undecodable": true }
    ]))
    .unwrap();
    let view = &mut state.accounts[0].mail;
    if let Some(open) = view.open.as_mut() {
        open.attachments = Some(list);
    }
    state
}

#[test]
fn attachments_save_open_and_refuse_as_they_should() {
    let mut state = with_attachments(opened("text/plain", PLAIN));
    let registry = commands::registry();
    // The command toggles the popup.
    registry
        .execute("mail.attachments", &mut state, Origin::Bus)
        .unwrap();
    assert!(state.accounts[0].mail.attachments_open);
    state.take_work();
    state.save_attachment("3", false);
    let work = state.take_work();
    assert!(
        work.iter().any(
            |w| matches!(w, Work::Attachment { part, name, open: false, blob, .. }
            if part == "3" && name == "install.sh" && blob == "B3")
        ),
        "{work:?}"
    );
    // A script is never opened; an undecodable part never fetched.
    state.save_attachment("3", true);
    state.save_attachment("4", false);
    assert!(state.take_work().is_empty());
    // Save All fetches what can be fetched.
    registry
        .execute("mail.save_attachments", &mut state, Origin::Bus)
        .unwrap();
    let parts: Vec<String> = state
        .take_work()
        .into_iter()
        .filter_map(|w| match w {
            Work::Attachment { part, .. } => Some(part),
            _ => None,
        })
        .collect();
    assert_eq!(parts, ["2", "3"]);
    // email.info lists them, with what became of each.
    let info = state.info()["accounts"][0]["mail"]["attachments"].clone();
    assert_eq!(info[1]["name"], "install.sh");
    assert_eq!(info[1]["save_only"], true);
    assert_eq!(info[2]["undecodable"], true);
    assert_eq!(info[0]["transfer"]["state"], "saving");
}

#[test]
fn reader_attachments_studio_dark() {
    let mut state = with_attachments(opened("text/plain", PLAIN));
    state.accounts[0].mail.attachments_open = true;
    state.accounts[0].mail.transfers.insert(
        "2".into(),
        email::state::Transfer::Saved("/home/user/Downloads/plan.pdf".into()),
    );
    let mut h = harness(state, &theme(Scheme::Studio, Mode::Dark));
    h.run();
    h.snapshot("reader_attachments_studio_dark");
}

/// A contact card for the completion tests.
fn card(name: &str, emails: &[&str]) -> jmap::Contact {
    let map: serde_json::Map<String, serde_json::Value> = emails
        .iter()
        .enumerate()
        .map(|(n, e)| (format!("e{n}"), serde_json::json!({ "address": e })))
        .collect();
    serde_json::from_value(serde_json::json!({
        "id": name,
        "addressBookId": "B1",
        "data": { "name": { "full": name }, "emails": map }
    }))
    .unwrap()
}

/// A compose open with `typed` in To and the account having seen Bob and
/// Annabel; the suggestion ticket the typing issued.
fn composing(typed: &str) -> (State, u64) {
    let mut state = opened("text/plain", PLAIN);
    state.accounts[0].mail.recent = vec![
        email::suggest::Suggestion {
            name: "Annabel Lee".into(),
            email: "annabel@y.invalid".into(),
            source: email::suggest::Source::Recent,
        },
        email::suggest::Suggestion {
            name: "Bob".into(),
            email: "bob@example.invalid".into(),
            source: email::suggest::Source::Recent,
        },
    ];
    commands::registry()
        .execute("mail.compose", &mut state, Origin::Bus)
        .unwrap();
    state.take_work();
    state.apply_ui(UiEvent::ComposeField(
        email::state::ComposeField::To,
        typed.into(),
    ));
    let ticket = state
        .compose
        .as_ref()
        .unwrap()
        .suggest
        .as_ref()
        .map_or(0, |s| s.ticket);
    (state, ticket)
}

#[test]
fn typing_suggests_recent_at_once_then_contacts_after_the_pause() {
    let (mut state, ticket) = composing("cy@example.invalid, ann");
    let suggest = state.compose.as_ref().unwrap().suggest.clone().unwrap();
    assert_eq!(suggest.query, "ann");
    assert_eq!(suggest.items.len(), 1, "the recent address that matches");
    assert!(matches!(state.take_work()[..], [Work::SuggestWait { ticket: t }] if t == ticket));
    // The pause passes: the contacts are looked up.
    state.apply(Event::SuggestDue(ticket));
    assert!(
        matches!(&state.take_work()[..], [Work::Suggest { ticket: t, text, .. }] if *t == ticket && text == "ann")
    );
    let home = tag(&state, "home");
    state.apply(Event::Account(
        home.clone(),
        Done::Suggested(
            ticket,
            Ok(vec![card(
                "Ann Example",
                &["ann@x.invalid", "ann@work.invalid"],
            )]),
        ),
    ));
    let items = state
        .compose
        .as_ref()
        .unwrap()
        .suggest
        .clone()
        .unwrap()
        .items;
    let emails: Vec<&str> = items.iter().map(|s| s.email.as_str()).collect();
    assert_eq!(
        emails,
        ["ann@x.invalid", "ann@work.invalid", "annabel@y.invalid"]
    );
    // Typing on makes the old answer late: it is dropped.
    state.apply_ui(UiEvent::ComposeField(
        email::state::ComposeField::To,
        "cy@example.invalid, anna".into(),
    ));
    state.apply(Event::Account(
        home,
        Done::Suggested(ticket, Ok(vec![card("Ann Example", &["ann@x.invalid"])])),
    ));
    let items = state
        .compose
        .as_ref()
        .unwrap()
        .suggest
        .clone()
        .unwrap()
        .items;
    assert_eq!(items.len(), 1, "only the newest lookup applies");
    // Taking one replaces only the typed entry.
    state.apply_ui(UiEvent::SuggestAccept(Some(0)));
    let compose = state.compose.as_ref().unwrap();
    assert_eq!(
        compose.to_text,
        "cy@example.invalid, Annabel Lee <annabel@y.invalid>, "
    );
    assert!(compose.suggest.is_none());
    // A single character shows nothing and looks nothing up.
    state.take_work();
    state.apply_ui(UiEvent::ComposeField(
        email::state::ComposeField::Cc,
        "a".into(),
    ));
    assert!(state.compose.as_ref().unwrap().suggest.is_none());
    assert!(state.take_work().is_empty());
}

#[test]
fn compose_suggestions_studio_dark() {
    let (mut state, ticket) = composing("an");
    let home = tag(&state, "home");
    state.apply(Event::Account(
        home,
        Done::Suggested(
            ticket,
            Ok(vec![
                card("Ann Example", &["ann@x.invalid", "ann@work.invalid"]),
                card("Smith, Anna", &["anna.smith@z.invalid"]),
            ]),
        ),
    ));
    let mut h = window(state, &theme(Scheme::Studio, Mode::Dark));
    h.ctx
        .memory_mut(|m| m.request_focus(egui::Id::new("compose-recipients").with("to")));
    h.run();
    h.snapshot("compose_suggestions_studio_dark");
}

#[test]
fn identities_dialog_studio_dark() {
    let (mut state, _) = composing("an");
    state.apply_ui(UiEvent::OpenIdentities("home".into()));
    state.apply_ui(UiEvent::IdentityAdd);
    state.apply_ui(UiEvent::IdentityEdit(
        email::state::IdentityField::Name,
        "Ann Work".into(),
    ));
    state.apply_ui(UiEvent::IdentityEdit(
        email::state::IdentityField::Address,
        "ann@work.invalid".into(),
    ));
    let mut h = window(state, &theme(Scheme::Studio, Mode::Dark));
    h.run();
    h.snapshot("identities_dialog_studio_dark");
}

fn snapshot(name: &str, state: State, theme: &Theme) {
    let mut h = harness(state, theme);
    h.run();
    h.snapshot(name);
}

#[test]
fn reader_markdown_studio_dark() {
    snapshot(
        "reader_markdown_studio_dark",
        opened("text/plain", MARKDOWN),
        &theme(Scheme::Studio, Mode::Dark),
    );
}

// Pro draws on its dark chrome in either mode: a hue scheme is the light
// picture.
#[test]
fn reader_markdown_ocean_light() {
    snapshot(
        "reader_markdown_ocean_light",
        opened("text/plain", MARKDOWN),
        &theme(Scheme::Ocean, Mode::Light),
    );
}

#[test]
fn reader_plain_ocean_light() {
    snapshot(
        "reader_plain_ocean_light",
        opened("text/plain", PLAIN),
        &theme(Scheme::Ocean, Mode::Light),
    );
}

#[test]
fn reader_light_content_studio_dark() {
    // The window is dark; only the message body is light, and the header
    // and toolbar keep the window's dark look.
    let mut state = opened("text/plain", PLAIN);
    state.reader_light = true;
    let mut h = harness(state, &theme(Scheme::Studio, Mode::Dark));
    h.run();
    h.snapshot("reader_light_content_studio_dark");
}

#[test]
fn reader_panel_menu_studio_dark() {
    // The reader's three dots open its menu, which holds Light content.
    let mut h = harness(
        opened("text/plain", PLAIN),
        &theme(Scheme::Studio, Mode::Dark),
    );
    h.get_by_label("Panel menu").click();
    h.run();
    // The menu opens on the frame after its button's click.
    h.run();
    h.snapshot("reader_panel_menu_studio_dark");
}

#[test]
fn light_content_is_only_the_body_and_the_window_stays_dark() {
    let mut state = opened("text/plain", PLAIN);
    state.apply_ui(UiEvent::SetReaderLight(true));
    assert!(state.reader_light);
    state.apply_ui(UiEvent::SetReaderLight(false));
    assert!(!state.reader_light);
}

#[test]
fn reader_reply_options_studio_dark() {
    let mut h = harness(
        opened("text/plain", PLAIN),
        &theme(Scheme::Studio, Mode::Dark),
    );
    h.get_by_label(&label("reply-options")).click();
    h.run();
    h.snapshot("reader_reply_options_studio_dark");
}

#[test]
fn message_menu_studio_dark() {
    let mut h = window(
        opened("text/plain", PLAIN),
        &theme(Scheme::Studio, Mode::Dark),
    );
    toolkit::menu::open_path(&h.ctx, &[label("message")]).unwrap();
    h.run();
    h.snapshot("message_menu_studio_dark");
}

#[test]
fn view_message_menu_studio_dark() {
    let mut h = window(
        opened("text/plain", MARKDOWN),
        &theme(Scheme::Studio, Mode::Dark),
    );
    toolkit::menu::open_path(&h.ctx, &[label("view"), label("message")]).unwrap();
    h.run();
    h.snapshot("view_message_menu_studio_dark");
}

#[test]
fn reader_context_menu_studio_dark() {
    let mut h = window(
        opened("text/plain", PLAIN),
        &theme(Scheme::Studio, Mode::Dark),
    );
    let at = h.get_by_label(PLAIN).rect().center();
    h.hover_at(at);
    h.run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    h.run();
    // The Message menu's rows, from the registry.
    assert!(h.query_by_label(&label("reply-all")).is_some());
    assert!(h.query_by_label_contains(&label("mark-unread")).is_some());
    h.snapshot("reader_context_menu_studio_dark");
}

#[test]
fn quoted_reply_compose_studio_dark() {
    let mut state = opened("text/plain", PLAIN);
    commands::registry()
        .execute("mail.reply_quoted", &mut state, Origin::Bus)
        .unwrap();
    let mut h = window(state, &theme(Scheme::Studio, Mode::Dark));
    h.run();
    h.snapshot("quoted_reply_compose_studio_dark");
}
