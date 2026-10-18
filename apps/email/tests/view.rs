// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! email's drawing, driven through kittest with no network: the account
//! tree's rows select their account, mode and folder, its keys move and
//! collapse, an unread email row opens and marks read, the no-accounts and
//! failed panes offer Email Settings, and offscreen snapshots of the
//! window. Regenerate the images with `UPDATE_SNAPSHOTS=1` and look at them
//! before committing.
use design::{DesignContext, Mode, Scheme};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use email::drafts::{DraftOp, SavedDraft};
use email::secrets::Secrets;
use email::shell::{Done, Event};
use email::state::{
    ComposeField, Confirm, Mode as Surface, Node, Phase, SessionInfo, State, Tag, UiState,
};
use email::view::{UiEvent, view};
use email::work::Work;
use email::{commands, label, strings};
use jmap::EmailPage;
use settings::email::{AccountEntry, Email as EmailSettings};
use toolkit::Theme;

/// The window over a fixture state; the state applies every frame's
/// events, so the test asserts on real state transitions.
struct Window {
    state: State,
    /// Every event the view reported, in order.
    seen: Vec<UiEvent>,
}

fn entry(id: &str, name: &str) -> AccountEntry {
    AccountEntry {
        id: id.into(),
        name: name.into(),
        url: format!("https://{id}.example.invalid"),
        email: format!("ann@{id}.example.invalid"),
        ca_path: None,
        enabled: true,
        signature: String::new(),
    }
}

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

/// Two accounts: Home connected with mail, a calendar and a book; Work
/// failed to connect.
fn fixture() -> State {
    let settings = EmailSettings {
        accounts: vec![entry("home", "Home"), entry("work", "Work")],
        ..EmailSettings::default()
    };
    let mut secrets = Secrets::default();
    secrets.set("home", "secret");
    secrets.set("work", "secret");
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
    let work = tag(&state, "work");
    state.apply(Event::Connected(
        work,
        Err(jmap::Error::Unauthorized("wrong password".into())),
    ));
    let folder = jmap::Mailbox {
        id: "m-lists".into(),
        name: "Lists".into(),
        ..jmap::Mailbox::default()
    };
    let child = jmap::Mailbox {
        id: "m-rust".into(),
        name: "Rust".into(),
        parent_id: Some("m-lists".into()),
        unread_emails: 3,
        ..jmap::Mailbox::default()
    };
    done(
        &mut state,
        "home",
        Done::Mailboxes(Ok(vec![
            jmap::Mailbox {
                id: "m-in".into(),
                name: "Inbox".into(),
                role: Some("inbox".into()),
                unread_emails: 1,
                ..jmap::Mailbox::default()
            },
            jmap::Mailbox {
                id: "m-sent".into(),
                name: "Sent".into(),
                role: Some("sent".into()),
                ..jmap::Mailbox::default()
            },
            folder,
            child,
        ])),
    );
    // The answer to the inbox's first page request (ticket 1).
    done(
        &mut state,
        "home",
        Done::Page(
            1,
            Ok(EmailPage {
                emails: vec![
                    serde_json::from_value(serde_json::json!({
                        "id": "E1",
                        "subject": "Lunch",
                        "from": [{ "name": "Bob", "email": "bob@example.invalid" }],
                        "receivedAt": "2026-10-13T01:02:03Z",
                        "keywords": {}
                    }))
                    .unwrap(),
                    serde_json::from_value(serde_json::json!({
                        "id": "E2",
                        "subject": "Minutes",
                        "from": [{ "name": "Cy", "email": "cy@example.invalid" }],
                        "receivedAt": "2026-10-12T05:00:00Z",
                        "keywords": { "$seen": true }
                    }))
                    .unwrap(),
                ],
                total: 2,
            }),
        ),
    );
    done(
        &mut state,
        "home",
        Done::Calendars(Ok(vec![jmap::Calendar {
            id: "C1".into(),
            name: "Main".into(),
            ..jmap::Calendar::default()
        }])),
    );
    done(
        &mut state,
        "home",
        Done::Books(Ok(vec![jmap::AddressBook {
            id: "B1".into(),
            name: "Colleagues".into(),
            ..jmap::AddressBook::default()
        }])),
    );
    state.take_work();
    state
}

fn empty() -> State {
    State::new(
        UiState::default(),
        Surface::Mail,
        None,
        EmailSettings::default(),
        Secrets::default(),
    )
}

fn theme(scheme: Scheme, mode: Mode) -> Theme {
    Theme::for_context(DesignContext {
        scheme,
        mode,
        ..DesignContext::default()
    })
}

fn harness_with(state: State, theme: &Theme) -> Harness<'static, Window> {
    // Dates render in UTC, whatever the worker's zone.
    // SAFETY: every test sets the same value before any date is drawn.
    unsafe { std::env::set_var("TZ", "UTC") };
    let registry = commands::registry();
    let strings = strings();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1180.0, 760.0))
        .build_ui_state(
            move |ui, window: &mut Window| {
                let events = view(ui, &window.state, &registry, &strings, 2.0);
                for event in events {
                    window.seen.push(event.clone());
                    window.state.apply_ui(event);
                }
            },
            Window {
                state,
                seen: Vec::new(),
            },
        );
    toolkit::install(&h.ctx, theme);
    h.run();
    h
}

fn harness(state: State) -> Harness<'static, Window> {
    harness_with(state, &theme(Scheme::Pro, Mode::Light))
}

#[test]
fn the_tree_draws_each_account_with_its_mailboxes_calendars_and_books() {
    let h = harness(fixture());
    // A selected row answers to two accessible nodes (the button and its
    // labelled widget info), so the draw asserts on "at least one".
    for name in [
        "Home",
        "Work",
        "Inbox",
        "Lists",
        "Rust",
        "Main",
        "Colleagues",
    ] {
        assert!(h.query_all_by_label(name).next().is_some(), "{name}");
    }
    assert!(h.query_all_by_label(&label("calendars")).next().is_some());
    assert!(
        h.query_all_by_label(&label("address-books"))
            .next()
            .is_some()
    );
}

#[test]
fn a_mailbox_row_click_selects_the_mailbox() {
    let mut h = harness(fixture());
    h.get_by_label("Sent").click();
    h.run();
    let state = &h.state().state;
    assert_eq!(
        state.account("home").unwrap().mail.selected.as_deref(),
        Some("m-sent")
    );
}

#[test]
fn a_calendar_row_switches_the_mode_and_an_account_row_the_account() {
    let mut h = harness(fixture());
    h.get_by_label("Main").click();
    h.run();
    assert_eq!(h.state().state.mode, Surface::Calendar);
    h.get_by_label("Work").click();
    h.run();
    let state = &h.state().state;
    assert_eq!(state.selected.as_deref(), Some("work"));
    // The failed account's pane: its error, a retry and Email Settings.
    assert!(h.query_all_by_label(&label("retry")).next().is_some());
    assert!(
        h.query_all_by_label(&label("open-settings"))
            .next()
            .is_some()
    );
}

#[test]
fn the_tree_takes_the_keyboard_after_a_click() {
    let mut h = harness(fixture());
    h.get_by_label("Lists").click();
    h.run();
    h.key_press(egui::Key::ArrowLeft);
    h.run();
    let state = &h.state().state;
    assert!(!state.expanded(&Node::Mailbox("home".into(), "m-lists".into())));
    assert!(h.query_all_by_label("Rust").next().is_none(), "collapsed");
    h.key_press(egui::Key::ArrowDown);
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    // Down from Lists is the Calendars group: Enter shows the calendar mode.
    assert_eq!(h.state().state.mode, Surface::Calendar);
}

#[test]
fn an_unread_email_row_opens_and_marks_read() {
    let mut h = harness(fixture());
    h.get_by_label("Bob").click();
    h.run();
    let state = &h.state().state;
    assert!(
        state.account("home").unwrap().mail.emails[0].is_read(),
        "the row marks read at once"
    );
    let work = h.state_mut().state.take_work();
    assert!(
        work.iter()
            .any(|w| matches!(w, Work::MarkRead { id, read: true, .. } if id == "E1"))
    );
    assert!(
        work.iter()
            .any(|w| matches!(w, Work::OpenEmail { id, .. } if id == "E1"))
    );
}

#[test]
fn the_contacts_filter_refetches() {
    let mut h = harness(fixture());
    h.state_mut().state.set_mode(Surface::Contacts);
    h.state_mut().state.take_work();
    h.run();
    let field = h.get_by_role(egui::accesskit::Role::TextInput);
    field.click();
    h.run();
    h.get_by_role(egui::accesskit::Role::TextInput)
        .type_text("bob");
    h.run();
    let state = &h.state().state;
    assert_eq!(state.account("home").unwrap().contacts.filter, "bob");
    assert!(
        h.state_mut()
            .state
            .take_work()
            .iter()
            .any(|w| matches!(w, Work::FetchContacts { filter, .. } if filter == "bob"))
    );
}

#[test]
fn no_accounts_offers_email_settings() {
    let mut h = harness(empty());
    h.get_by_label(&label("open-settings")).click();
    h.run();
    assert!(h.state().seen.contains(&UiEvent::OpenPrefs));
    assert!(
        h.state_mut()
            .state
            .take_work()
            .iter()
            .any(|w| matches!(w, Work::OpenPrefs))
    );
}

#[test]
fn a_failed_account_retries() {
    let mut state = fixture();
    state.select_account("work");
    assert!(matches!(
        state.account("work").unwrap().phase,
        Phase::Failed(_)
    ));
    let mut h = harness(state);
    h.get_by_label(&label("retry")).click();
    h.run();
    let state = &mut h.state_mut().state;
    assert_eq!(state.account("work").unwrap().phase, Phase::Connecting);
    assert!(
        state
            .take_work()
            .iter()
            .any(|w| matches!(w, Work::Connect { account, .. } if account == "work"))
    );
}

fn snapshot(name: &str, state: State, theme: &Theme) {
    let mut h = harness_with(state, theme);
    h.run();
    h.snapshot(name);
}

#[test]
fn window_pro_light() {
    snapshot(
        "window_pro_light",
        fixture(),
        &theme(Scheme::Pro, Mode::Light),
    );
}

#[test]
fn window_studio_dark() {
    snapshot(
        "window_studio_dark",
        fixture(),
        &theme(Scheme::Studio, Mode::Dark),
    );
}

#[test]
fn failed_account_pro_light() {
    let mut state = fixture();
    state.select_account("work");
    snapshot(
        "failed_account_pro_light",
        state,
        &theme(Scheme::Pro, Mode::Light),
    );
}

#[test]
fn new_event_form_studio_dark() {
    let mut state = fixture();
    state.new_event();
    if let Some(email::state::Form::Event(form)) = &mut state.form {
        // A fixed time, so the picture does not move with the clock.
        form.title = "Standup".into();
        form.start = "2026-10-17T09:00:00+10:00".into();
        form.end = "2026-10-17T09:15:00+10:00".into();
    }
    snapshot(
        "new_event_form_studio_dark",
        state,
        &theme(Scheme::Studio, Mode::Dark),
    );
}

#[test]
fn confirm_delete_pro_light() {
    let mut state = fixture();
    let first = state.accounts[0].mail.emails[0].clone();
    state.accounts[0].mail.open = Some(first);
    state.delete_open();
    assert!(matches!(
        state.form,
        Some(email::state::Form::Delete { .. })
    ));
    snapshot(
        "confirm_delete_pro_light",
        state,
        &theme(Scheme::Pro, Mode::Light),
    );
}

#[test]
fn no_accounts_studio_dark() {
    snapshot(
        "no_accounts_studio_dark",
        empty(),
        &theme(Scheme::Studio, Mode::Dark),
    );
}

/// The fixture with a compose open on Home and something typed in it.
fn typing() -> State {
    let mut state = fixture();
    state.apply_ui(UiEvent::Compose);
    for (field, value) in [
        (ComposeField::To, "bob@example.invalid"),
        (ComposeField::Subject, "Lunch on Friday"),
        (ComposeField::Body, "Noon at the usual place?"),
    ] {
        state.apply_ui(UiEvent::ComposeField(field, value.into()));
    }
    state
}

#[test]
fn escape_on_a_typed_compose_asks_and_keeps_the_draft() {
    let mut h = harness(typing());
    h.key_press(egui::Key::Escape);
    h.run();
    let state = &h.state().state;
    assert_eq!(state.confirm, Some(Confirm::CloseCompose));
    assert!(state.compose.is_some(), "Escape never drops what is typed");
    // Escape again is Keep Editing: the question goes, the compose stays.
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(h.state().state.confirm.is_none());
    assert!(h.state().state.compose.is_some());
    // Cancel asks; Keep Draft, the default, closes and keeps it.
    h.get_by_label(&label("cancel")).click();
    h.run();
    assert_eq!(h.state().state.confirm, Some(Confirm::CloseCompose));
    h.get_by_label(&label("keep-draft")).click();
    h.run();
    let state = &mut h.state_mut().state;
    assert!(state.compose.is_none() && state.confirm.is_none());
    let kept = state.take_draft_ops();
    assert!(
        kept.iter()
            .any(|op| matches!(op, DraftOp::Save(d) if d.body == "Noon at the usual place?")),
        "{kept:?}"
    );
    assert!(!kept.iter().any(|op| matches!(op, DraftOp::Remove(_))));
}

#[test]
fn confirm_close_draft_studio_dark() {
    let mut state = typing();
    state.apply_ui(UiEvent::CloseCompose);
    assert_eq!(state.confirm, Some(Confirm::CloseCompose));
    snapshot(
        "confirm_close_draft_studio_dark",
        state,
        &theme(Scheme::Studio, Mode::Dark),
    );
}

#[test]
fn confirm_quit_draft_pro_light() {
    let mut state = typing();
    assert!(!state.request_quit());
    snapshot(
        "confirm_quit_draft_pro_light",
        state,
        &theme(Scheme::Pro, Mode::Light),
    );
}

#[test]
fn restore_draft_studio_dark() {
    let mut state = fixture();
    let kept = |id: &str, subject: &str, saved_at: &str| SavedDraft {
        schema_version: email::drafts::CURRENT_SCHEMA,
        id: id.into(),
        account: "home".into(),
        from: "ann@home.example.invalid".into(),
        to: "bob@example.invalid, ".into(),
        subject: subject.into(),
        body: "Noon at the usual place?".into(),
        saved_at: saved_at.into(),
        ..SavedDraft::default()
    };
    state.offer_drafts(vec![
        kept("0b", "Lunch on Friday", "2026-10-17T23:40:00Z"),
        kept("0a", "Minutes", "2026-10-16T08:00:00Z"),
    ]);
    let mut h = harness_with(state, &theme(Scheme::Studio, Mode::Dark));
    h.run();
    h.snapshot("restore_draft_studio_dark");
    h.get_by_label(&label("restore-draft")).click();
    h.run();
    let state = &h.state().state;
    let compose = state.compose.as_ref().expect("restored");
    assert_eq!(compose.draft.subject, "Lunch on Friday");
    assert_eq!(state.restore.len(), 1, "the next waits its turn");
}
