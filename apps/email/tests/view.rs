// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! email's drawing, driven through kittest with no network: the places
//! panel's mailbox row selects, an unread email row opens and marks read,
//! and the setup form's Connect runs with what the form holds.
use design::{DesignContext, Mode, Scheme};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use email::shell::Event;
use email::state::{Mode, SessionInfo, State, UiState};
use email::view::{UiEvent, view};
use email::{commands, label, strings};
use jmap::{Account, EmailPage};
use toolkit::Theme;

/// The window over a fixture state; the state applies every frame's
/// events, so the test asserts on real state transitions.
struct Window {
    state: State,
}

fn account() -> Account {
    Account {
        url: "https://example.invalid".into(),
        email: "ann@example.invalid".into(),
        password: "secret".into(),
        ca_pem: None,
    }
}

fn fixture() -> State {
    let mut state = State::new(Some(account()), UiState::default());
    state.apply(Event::Connected(Ok(SessionInfo {
        account_name: "Account 7".into(),
        account_id: "7".into(),
    })));
    state.apply(Event::Mailboxes(Ok(vec![
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
    ])));
    state.apply(Event::Page(Ok(EmailPage {
        emails: vec![serde_json::from_value(serde_json::json!({
            "id": "E1",
            "subject": "Lunch",
            "from": [{ "name": "Bob", "email": "bob@example.invalid" }],
            "receivedAt": "2026-10-13T01:02:03Z",
            "keywords": {}
        }))
        .unwrap()],
        total: 1,
    })));
    state.apply(Event::Calendars(Ok(vec![jmap::Calendar {
        id: "C1".into(),
        name: "Main".into(),
        ..jmap::Calendar::default()
    }])));
    state.apply(Event::Books(Ok(vec![jmap::AddressBook {
        id: "B1".into(),
        name: "Colleagues".into(),
        ..jmap::AddressBook::default()
    }])));
    state
}

fn harness(state: State) -> Harness<'static, Window> {
    let registry = commands::registry();
    let strings = strings();
    let theme = Theme::for_context(DesignContext {
        scheme: Scheme::Pro,
        mode: Mode::Light,
        ..DesignContext::default()
    });
    let mut h = Harness::builder()
        .with_size(egui::vec2(1180.0, 760.0))
        .build_ui_state(
            move |ui, window: &mut Window| {
                let events = view(ui, &window.state, &registry, &strings, 2.0);
                for event in events {
                    window.state.apply_ui(event);
                }
            },
            Window { state },
        );
    toolkit::install(&h.ctx, &theme);
    h.run();
    h
}

#[test]
fn the_window_draws_the_account_and_the_surfaces() {
    let h = harness(fixture());
    assert!(h.state().session.is_some());
    // The places panel shows the mailbox, the calendar and the book.
    let _ = h.get_by_label("Inbox");
    let _ = h.get_by_label("Main");
    let _ = h.get_by_label("Colleagues");
}

#[test]
fn a_mailbox_row_click_selects_the_mailbox() {
    let mut h = harness(fixture());
    h.get_by_label("Sent").click();
    h.run();
    assert_eq!(h.state().state.mail.selected.as_deref(), Some("m-sent"));
}

#[test]
fn an_unread_email_row_opens_and_marks_read() {
    let mut h = harness(fixture());
    h.get_by_label("Bob").click();
    h.run();
    let state = &h.state().state;
    assert!(state.mail.emails[0].is_read(), "the row marks read at once");
    let work = state.take_work();
    assert!(work.iter().any(
        |w| matches!(w, email::work::Work::MarkRead { id, read: true } if id == "E1")
    ));
    assert!(work
        .iter()
        .any(|w| matches!(w, email::work::Work::OpenEmail { id } if id == "E1")));
}

#[test]
fn the_contacts_filter_refetches() {
    let mut h = harness(fixture());
    h.state().state.mode = Mode::Contacts;
    h.run();
    let filter = h.get_by_label(&label("contact-search"));
    filter.set_text("bob");
    h.run();
    assert_eq!(h.state().state.contacts.filter, "bob");
    assert!(h
        .state()
        .state
        .take_work()
        .iter()
        .any(|w| matches!(w, email::work::Work::FetchContacts { filter, .. } if filter == "bob")));
}

#[test]
fn the_setup_form_connect_runs_with_the_forms_account() {
    let mut state = State::new(None, UiState::default());
    state.setup.url = "https://example.invalid".into();
    state.setup.email = "ann@example.invalid".into();
    state.setup.password = "secret".into();
    let mut h = harness(state);
    h.get_by_label(&label("setup-connect")).click();
    h.run();
    let window = h.state();
    assert!(window.state.account().is_some());
    assert!(matches!(
        window.state.take_work().as_slice(),
        [email::work::Work::Connect(_)]
    ));
}
