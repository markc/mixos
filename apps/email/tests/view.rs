// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! email's drawing, driven through kittest with no network: the account
//! tree's rows select their account, mode and folder, its keys move and
//! collapse, the message list selects (click, Ctrl and Shift clicks,
//! Ctrl+A, select all, Escape) without opening, its headers sort and its
//! pager pages through server queries, an unread email row opens
//! on a double-click or Enter and marks read, the no-accounts and
//! failed panes offer Email Settings, and offscreen snapshots of the
//! window. Regenerate the images with `UPDATE_SNAPSHOTS=1` and look at them
//! before committing.
use design::{DesignContext, Mode, Scheme};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
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
        identities: Vec::new(),
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
    // Friday 2026-10-16 09:30 UTC: the list's dates do not move with the
    // clock.
    email::view::mail::pin_clock(&h.ctx, NOW, 0);
    h.run();
    h
}

/// The pinned clock, in Unix seconds.
const NOW: i64 = 1_792_143_000;

/// The fixture's inbox page replaced by six emails: a long sender and a
/// long subject to elide, unread, flagged and attachment markers, and a
/// date from today, this week, this year and an older year.
fn long_list() -> State {
    let mut state = fixture();
    let email = |id: &str,
                 from: serde_json::Value,
                 subject: &str,
                 at: &str,
                 keywords: serde_json::Value,
                 attachment: bool|
     -> jmap::Email {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "subject": subject,
            "from": from,
            "receivedAt": at,
            "keywords": keywords,
            "hasAttachment": attachment,
        }))
        .unwrap()
    };
    let emails = vec![
        email(
            "L1",
            serde_json::json!([{ "name": "Ann Lee", "email": "ann@example.invalid" }]),
            "Lunch today?",
            "2026-10-16T08:05:00Z",
            serde_json::json!({}),
            false,
        ),
        email(
            "L2",
            serde_json::json!([{ "name": "Bartholomew Rivera-Montgomery of the Extremely Long Display Name Society", "email": "bart@example.invalid" }]),
            "Quarterly report: the full numbers, every appendix, the board's comments on each, and a summary of the summary",
            "2026-10-14T14:02:00Z",
            serde_json::json!({ "$seen": true, "$flagged": true }),
            true,
        ),
        email(
            "L3",
            serde_json::json!([{
                "email": "cy.no-display-name.but-a-very-long-local-part@subdomain.example.invalid"
            }]),
            "Minutes",
            "2026-10-12T17:45:00Z",
            serde_json::json!({ "$seen": true }),
            false,
        ),
        email(
            "L4",
            serde_json::json!([{ "name": "Dee", "email": "dee@example.invalid" }]),
            "Re: build cluster",
            "2026-08-03T09:00:00Z",
            serde_json::json!({ "$flagged": true }),
            false,
        ),
        email(
            "L5",
            serde_json::json!([{ "name": "Eve", "email": "eve@example.invalid" }]),
            "Photos from the weekend",
            "2026-01-16T11:00:00Z",
            serde_json::json!({ "$seen": true }),
            true,
        ),
        email(
            "L6",
            serde_json::json!([{ "name": "Gus", "email": "gus@example.invalid" }]),
            "Old thread",
            "2025-10-16T10:00:00Z",
            serde_json::json!({ "$seen": true }),
            false,
        ),
    ];
    let view = &mut state.accounts[0];
    view.mail.total = 236;
    view.mail.emails = emails;
    view.mail.table.selection = vec!["L3".into()];
    view.mail.table.focus = Some(2);
    state
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

/// Two clicks on the node labelled `label` in one frame: inside the
/// double-click window whatever the harness's step.
fn double_click(h: &mut Harness<'static, Window>, label: &str) {
    let row = h.get_by_label_contains(label).rect().center();
    h.hover_at(row);
    h.run();
    let clicks = (0..2).flat_map(|_| {
        [true, false].map(|pressed| egui::Event::PointerButton {
            pos: row,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        })
    });
    h.input_mut().events.extend(clicks);
    h.run();
}

#[test]
fn a_single_click_opens_and_marks_read_and_a_double_click_does_not_reopen() {
    let mut h = harness(fixture());
    // A row's accessible label is its cells' text: "Lunch, Bob, Tue 01:02".
    // One click selects the row and opens it.
    h.get_by_label_contains("Bob").click();
    h.run();
    let work = h.state_mut().state.take_work();
    assert!(
        work.iter()
            .any(|w| matches!(w, Work::OpenEmail { id, .. } if id == "E1"))
    );
    assert!(
        work.iter()
            .any(|w| matches!(w, Work::MarkRead { id, read: true, .. } if id == "E1"))
    );
    let mail = &h.state().state.account("home").unwrap().mail;
    assert!(
        mail.emails[0].is_read(),
        "the opened row marks read at once"
    );
    assert_eq!(mail.table.selection, ["E1"], "the opened row is selected");
    // A double-click on the row that is already open does not open it again.
    double_click(&mut h, "Bob");
    let again = h.state_mut().state.take_work();
    assert!(
        again.iter().all(|w| !matches!(w, Work::OpenEmail { .. })),
        "already open: {again:?}"
    );
}

fn right_click(h: &mut Harness<'static, Window>, label: &str) {
    let row = h.get_by_label_contains(label).rect().center();
    h.hover_at(row);
    h.run();
    let clicks = [true, false].map(|pressed| egui::Event::PointerButton {
        pos: row,
        button: egui::PointerButton::Secondary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    });
    h.input_mut().events.extend(clicks);
    h.run();
    h.run();
}

#[test]
fn a_right_click_on_a_row_opens_the_message_menu_for_that_email() {
    let mut h = harness(fixture());
    assert!(h.query_by_label_contains("Send as New").is_none());
    // A row outside the selection becomes it, and opens.
    right_click(&mut h, "Minutes");
    let ticket = h
        .state_mut()
        .state
        .take_work()
        .iter()
        .find_map(|w| match w {
            Work::OpenEmail { ticket, id, .. } if id == "E2" => Some(*ticket),
            _ => None,
        })
        .expect("the right-clicked row opens");
    assert_eq!(
        h.state()
            .state
            .account("home")
            .unwrap()
            .mail
            .table
            .selection,
        ["E2"]
    );
    // The menu is up; what acts on the open email waits for E2 to load.
    let entry = |h: &Harness<'static, Window>| {
        h.get_by_label_contains("Send as New")
            .accesskit_node()
            .is_disabled()
    };
    assert!(entry(&h), "disabled while E2 loads");
    let e2 = serde_json::json!({
        "id": "E2",
        "subject": "Minutes",
        "from": [{ "name": "Cy", "email": "cy@example.invalid" }],
        "receivedAt": "2026-10-12T05:00:00Z",
        "keywords": { "$seen": true }
    });
    done(
        &mut h.state_mut().state,
        "home",
        Done::Opened(
            ticket,
            None,
            Box::new(Ok(serde_json::from_value(e2).unwrap())),
        ),
    );
    h.run();
    assert!(!entry(&h), "enabled once E2 is open");
    h.get_by_label_contains("Send as New").click();
    h.run();
    assert!(
        h.state().seen.contains(&UiEvent::Command(
            "mail.send_as_new",
            toolkit::command::Origin::Menu
        )),
        "{:?}",
        h.state().seen
    );
}

/// The message list's rows, top to bottom, by their accessible labels.
fn list_rows(h: &Harness<'static, Window>) -> Vec<String> {
    let mut rows: Vec<(f32, String)> = h
        .query_all_by_role(egui::accesskit::Role::Row)
        .map(|n| {
            (
                n.rect().top(),
                n.accesskit_node().label().unwrap_or_default(),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, label)| label).collect()
}

#[test]
fn each_email_is_one_row_of_subject_from_and_a_compact_date() {
    let h = harness(long_list());
    assert_eq!(
        list_rows(&h),
        [
            "Lunch today?, Ann Lee, 08:05",
            "Quarterly report: the full numbers, every appendix, the board's comments on each, and a summary of the summary, Bartholomew Rivera-Montgomery of the Extremely Long Display Name Society, Wed 14:02",
            "Minutes, cy.no-display-name.but-a-very-long-local-part@subdomain.example.invalid, Mon 17:45",
            "Re: build cluster, Dee, 3 Aug",
            "Photos from the weekend, Eve, 16 Jan",
            "Old thread, Gus, 16/10/2025",
        ]
    );
    // Subject is the first column.
    let subject = h.get_by_label(&label("column-subject")).rect();
    let from = h.get_by_label(&label("column-from")).rect();
    let date = h.get_by_label(&label("column-date")).rect();
    assert!(subject.left() < from.left() && from.left() < date.left());
}

#[test]
fn dates_show_in_the_clock_zone() {
    // Ten hours east: Bob's 01:02 UTC Tuesday is 11:02 there.
    let mut h = harness(fixture());
    email::view::mail::pin_clock(&h.ctx, NOW, 10 * 3600);
    h.run();
    assert!(
        list_rows(&h)[0].ends_with("Tue 11:02"),
        "{:?}",
        list_rows(&h)
    );
}

fn opened(h: &mut Harness<'static, Window>) -> Vec<String> {
    h.state_mut()
        .state
        .take_work()
        .into_iter()
        .filter_map(|w| match w {
            Work::OpenEmail { id, .. } => Some(id),
            _ => None,
        })
        .collect()
}

#[test]
fn up_and_down_move_the_selection_and_enter_opens() {
    let mut h = harness(long_list());
    // A row click takes the keyboard, selects the row and opens it.
    h.get_by_label_contains("Minutes").click();
    h.run();
    assert_eq!(opened(&mut h), ["L3"]);
    // Down moves the keyboard's row and opens it, as a click does.
    h.key_press(egui::Key::ArrowDown);
    h.run();
    assert_eq!(opened(&mut h), ["L4"]);
    let mail = &h.state().state.account("home").unwrap().mail;
    assert_eq!(mail.table.selection, ["L4"]);
    h.key_press(egui::Key::ArrowUp);
    h.run();
    h.key_press(egui::Key::ArrowUp);
    h.run();
    assert_eq!(opened(&mut h), ["L3", "L2"], "each row Up reaches opens");
    let mail = &h.state().state.account("home").unwrap().mail;
    assert_eq!(mail.table.selection, ["L2"]);
    assert_eq!(mail.table.focus, Some(1));
    // Enter on the row already open does not open it again.
    h.key_press(egui::Key::Enter);
    h.run();
    assert!(
        opened(&mut h).is_empty(),
        "the open row is not asked for twice"
    );
}

#[test]
fn reader_panel_menu_opens_on_the_dots_and_holds_light_content() {
    let mut h = harness_with(fixture(), &theme(Scheme::Studio, Mode::Dark));
    assert!(
        h.query_by_label_contains("Light content").is_none(),
        "closed at first"
    );
    // Each pane has its dots; the reader's are the rightmost.
    h.query_all_by_label("Panel menu")
        .max_by(|a, b| a.rect().left().total_cmp(&b.rect().left()))
        .expect("the reader's dots")
        .click();
    h.run();
    h.run();
    h.run();
    assert!(
        h.query_by_label_contains("Light content").is_some(),
        "open after the dots"
    );
    h.snapshot("reader_panel_menu_studio_dark");
}

#[test]
fn tab_moves_between_the_three_panes_and_the_arrows_follow() {
    use email::state::Pane;
    let mut h = harness(long_list());
    h.get_by_label_contains("Minutes").click();
    h.run();
    assert_eq!(h.state().state.pane, Pane::List);
    // Tab goes list, reader, tree, list: the order the panes sit in.
    h.key_press(egui::Key::Tab);
    h.run();
    assert_eq!(h.state().state.pane, Pane::Reader);
    h.key_press(egui::Key::Tab);
    h.run();
    assert_eq!(h.state().state.pane, Pane::Tree);
    h.key_press(egui::Key::Tab);
    h.run();
    assert_eq!(h.state().state.pane, Pane::List);
    // Back in the list, Down moves through it again.
    h.key_press(egui::Key::ArrowDown);
    h.run();
    let mail = &h.state().state.account("home").unwrap().mail;
    assert_eq!(mail.table.selection, ["L4"]);
}

#[test]
fn left_and_right_do_nothing_in_the_list_and_up_and_down_still_move() {
    let mut h = harness(long_list());
    h.get_by_label_contains("Minutes").click();
    h.run();
    // Left and Right neither move the focus away nor change the selection.
    h.key_press(egui::Key::ArrowRight);
    h.run();
    h.key_press(egui::Key::ArrowLeft);
    h.run();
    let mail = &h.state().state.account("home").unwrap().mail;
    assert_eq!(mail.table.selection, ["L3"]);
    // Down still moves through the list.
    h.key_press(egui::Key::ArrowDown);
    h.run();
    let mail = &h.state().state.account("home").unwrap().mail;
    assert_eq!(mail.table.selection, ["L4"]);
}

#[test]
fn ctrl_and_shift_clicks_ctrl_a_and_escape_select_without_opening() {
    let mut h = harness(long_list());
    // A plain click opens its row; the selection gestures below do not.
    h.get_by_label_contains("Lunch today").click();
    h.run();
    assert_eq!(opened(&mut h), ["L1"]);
    // Shift+click: the range from the row clicked first.
    h.get_by_label_contains("Re: build cluster")
        .click_modifiers(egui::Modifiers::SHIFT);
    h.run();
    let mail = &h.state().state.account("home").unwrap().mail;
    assert_eq!(mail.table.selection, ["L1", "L2", "L3", "L4"]);
    assert!(h.query_by_label("4 selected").is_some());
    // Ctrl+click takes one out.
    h.get_by_label_contains("Minutes")
        .click_modifiers(egui::Modifiers::COMMAND);
    h.run();
    let mail = &h.state().state.account("home").unwrap().mail;
    assert_eq!(mail.table.selection, ["L1", "L2", "L4"]);
    // Ctrl+A: the whole page, and the offer of the whole mailbox.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run();
    let mail = &h.state().state.account("home").unwrap().mail;
    assert_eq!(mail.table.selection.len(), 6);
    assert!(h.query_by_label("6 selected").is_some());
    assert!(opened(&mut h).is_empty(), "selecting opens nothing");
    // Escape: back to the keyboard's row (the one Ctrl+clicked).
    h.key_press(egui::Key::Escape);
    h.run();
    let mail = &h.state().state.account("home").unwrap().mail;
    assert_eq!(mail.table.selection, ["L3"]);
    assert!(h.query_by_label("Select all 236 in folder").is_none());
    // Ctrl+A again, then "Select all 236 in folder": every email, counted
    // from the list's total with no query.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run();
    h.get_by_label("Select all 236 in folder").click();
    h.run();
    let mail = &h.state().state.account("home").unwrap().mail;
    assert!(mail.everything);
    assert_eq!(mail.selection_count(), 236);
    assert!(h.query_by_label("236 selected").is_some());
    assert!(h.query_by_label("All 236 selected").is_some());
    assert!(h.state_mut().state.take_work().is_empty());
    // A plain click is a selection of one again.
    h.get_by_label_contains("Photos").click();
    h.run();
    let mail = &h.state().state.account("home").unwrap().mail;
    assert!(!mail.everything);
    assert_eq!(mail.table.selection, ["L5"]);
    assert!(h.query_by_label("236 selected").is_none());
}

/// The one page fetch the state queued: its ticket, sort and position.
fn fetched(h: &mut Harness<'static, Window>) -> (u64, jmap::EmailSort, u64) {
    let fetches: Vec<(u64, jmap::EmailSort, u64)> = h
        .state_mut()
        .state
        .take_work()
        .into_iter()
        .filter_map(|w| match w {
            Work::FetchEmails {
                ticket,
                sort,
                position,
                ..
            } => Some((ticket, sort, position)),
            _ => None,
        })
        .collect();
    assert_eq!(fetches.len(), 1, "{fetches:?}");
    fetches[0]
}

/// The header labelled `label`'s sort arrow, as agents read it.
fn sort_of(h: &Harness<'static, Window>, label: &str) -> Option<egui::accesskit::SortDirection> {
    h.get_by_label(label).accesskit_node().sort_direction()
}

#[test]
fn a_header_click_sorts_on_the_server_ascending_first_then_flips() {
    use egui::accesskit::SortDirection;
    use jmap::{EmailSort, SortBy};
    let mut h = harness(long_list());
    let (subject, from, date) = (
        label("column-subject"),
        label("column-from"),
        label("column-date"),
    );
    // Newest first until a header is clicked.
    assert_eq!(sort_of(&h, &date), Some(SortDirection::Descending));
    assert_eq!(sort_of(&h, &subject), None);
    h.get_by_label(&subject).click();
    h.run();
    assert_eq!(
        fetched(&mut h).1,
        EmailSort::new(SortBy::Subject, true),
        "ascending first"
    );
    assert_eq!(sort_of(&h, &subject), Some(SortDirection::Ascending));
    assert_eq!(sort_of(&h, &date), None);
    let mail = &h.state().state.account("home").unwrap().mail;
    assert!(mail.table.selection.is_empty(), "the sort clears L3");
    h.get_by_label(&subject).click();
    h.run();
    assert_eq!(fetched(&mut h).1, EmailSort::new(SortBy::Subject, false));
    assert_eq!(sort_of(&h, &subject), Some(SortDirection::Descending));
    h.get_by_label(&from).click();
    h.run();
    assert_eq!(fetched(&mut h).1, EmailSort::new(SortBy::From, true));
    h.get_by_label(&date).click();
    h.run();
    assert_eq!(fetched(&mut h).1, EmailSort::new(SortBy::ReceivedAt, true));
    assert_eq!(sort_of(&h, &date), Some(SortDirection::Ascending));
}

#[test]
fn the_pager_pages_on_the_server_and_select_all_works_on_any_page() {
    let mut h = harness(long_list());
    assert!(h.query_by_label("1–6 of 236").is_some());
    assert!(
        h.get_by_label("Previous page")
            .accesskit_node()
            .is_disabled()
    );
    h.get_by_label("Next page").click();
    h.run();
    let (ticket, _, position) = fetched(&mut h);
    assert_eq!(position, 100);
    let mail = &h.state().state.account("home").unwrap().mail;
    assert_eq!(mail.table.page, 1);
    assert!(mail.table.selection.is_empty(), "the page clears L3");
    let email = |id: &str, subject: &str| -> jmap::Email {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "subject": subject,
            "from": [{ "name": "Hal", "email": "hal@example.invalid" }],
            "receivedAt": "2026-07-01T10:00:00Z",
            "keywords": { "$seen": true }
        }))
        .unwrap()
    };
    done(
        &mut h.state_mut().state,
        "home",
        Done::Page(
            ticket,
            Ok(EmailPage {
                emails: vec![email("Q1", "Page two first"), email("Q2", "Page two last")],
                total: 236,
            }),
        ),
    );
    h.run();
    assert!(h.query_by_label("101–102 of 236").is_some());
    assert_eq!(list_rows(&h).len(), 2);
    // Ctrl+A and "Select all 236 in folder" on the second page.
    h.get_by_label_contains("Page two first").click();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run();
    h.get_by_label("Select all 236 in folder").click();
    h.run();
    let mail = &h.state().state.account("home").unwrap().mail;
    assert!(mail.everything);
    assert_eq!(mail.selection_count(), 236);
    assert!(h.query_by_label("All 236 selected").is_some());
    // Last and First, then Previous from the last page.
    h.get_by_label("Last page").click();
    h.run();
    assert_eq!(fetched(&mut h).2, 200);
    assert!(!h.state().state.account("home").unwrap().mail.everything);
    h.get_by_label("First page").click();
    h.run();
    assert_eq!(fetched(&mut h).2, 0);
}

#[test]
fn an_email_opened_elsewhere_shows_selected_and_a_new_mailbox_clears_it() {
    let mut state = fixture();
    let first = state.accounts[0].mail.emails[1].clone();
    state.accounts[0].mail.open = Some(first);
    let table = state.accounts[0].mail.list_table();
    assert_eq!(table.selection, ["E2"]);
    assert_eq!(table.focus, Some(1));
    state.open_email("home", "E1".into());
    assert_eq!(state.accounts[0].mail.table.selection, ["E1"]);
    let mut h = harness(state);
    h.get_by_label("Sent").click();
    h.run();
    let mail = &h.state().state.account("home").unwrap().mail;
    assert!(mail.table.selection.is_empty() && mail.table.focus.is_none());
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

#[test]
fn the_status_bar_shows_the_current_accounts_own_connection() {
    let mut state = fixture();
    state.select_account("work");
    let mut h = harness(state);
    h.run();
    let failed = format!("Work — {}", label("not-connected"));
    assert!(h.query_by_label(&failed).is_some(), "{failed}");
    let wrong = format!("Work — {}", label("connected"));
    assert!(
        h.query_by_label_contains(&wrong).is_none(),
        "another account's connection never shows on this one"
    );
    h.state_mut().state.select_account("home");
    h.run();
    let ready = format!("Home — {}", label("connected"));
    assert!(h.query_by_label(&ready).is_some(), "{ready}");
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
fn mail_list_studio_light() {
    snapshot(
        "mail_list_studio_light",
        long_list(),
        &theme(Scheme::Studio, Mode::Light),
    );
}

#[test]
fn mail_list_studio_dark() {
    snapshot(
        "mail_list_studio_dark",
        long_list(),
        &theme(Scheme::Studio, Mode::Dark),
    );
}

/// `long_list` shown as the second page of 236, sorted by `column` in
/// `order` (its rows in that order, as the server would send them): the
/// header's arrow and the pager's "101–106 of 236".
fn sorted_page(column: &str, order: toolkit::datatable::Order) -> State {
    let mut state = long_list();
    let view = &mut state.accounts[0];
    let text = |e: &jmap::Email| match column {
        "from" => e.from_display().to_lowercase(),
        _ => e.subject.clone().unwrap_or_default().to_lowercase(),
    };
    view.mail
        .emails
        .sort_by(|a, b| order.apply(text(a).cmp(&text(b))));
    view.mail.table.sort = Some(toolkit::datatable::Sort::new(column, order));
    view.mail.table.page = 1;
    view.mail.table.selection.clear();
    view.mail.table.focus = None;
    state
}

/// Page 2 by Subject, A to Z.
#[test]
fn mail_list_paged_studio_light() {
    snapshot(
        "mail_list_paged_studio_light",
        sorted_page("subject", toolkit::datatable::Order::Ascending),
        &theme(Scheme::Studio, Mode::Light),
    );
}

/// Page 2 by From, Z to A.
#[test]
fn mail_list_paged_studio_dark() {
    snapshot(
        "mail_list_paged_studio_dark",
        sorted_page("from", toolkit::datatable::Order::Descending),
        &theme(Scheme::Studio, Mode::Dark),
    );
}

/// The whole page selected: the bar under the list counts it and offers
/// the mailbox's 236.
#[test]
fn mail_list_selected_pro_light() {
    let mut state = long_list();
    let view = &mut state.accounts[0];
    view.mail.table.selection = view.mail.keys();
    snapshot(
        "mail_list_selected_pro_light",
        state,
        &theme(Scheme::Pro, Mode::Light),
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
    state.delete_target();
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

/// `long_list` with L1, L2 and L4 selected (Ctrl+click), L3 the keyboard
/// row and nothing open.
fn three_selected() -> State {
    let mut state = long_list();
    state.accounts[0].mail.table.selection = vec!["L1".into(), "L2".into(), "L4".into()];
    state
}

#[test]
fn the_target_line_names_the_selection_or_this_email() {
    // One email open is no note: the reader's actions obviously apply to it.
    let h = harness(long_list());
    assert!(h.query_by_label("Applies to this email").is_none());
    let h = harness(three_selected());
    assert!(h.query_by_label("Applies to 3 selected").is_some());
    assert!(h.query_by_label("Applies to this email").is_none());
    // Nothing selected, no keyboard row, nothing open: no line.
    let h = harness(fixture());
    assert!(h.query_by_label_contains("Applies to").is_none());
}

#[test]
fn escape_cancels_a_select_all_fetch_and_changes_nothing() {
    let mut h = harness(long_list());
    h.get_by_label_contains("Lunch today").click();
    h.run();
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run();
    h.get_by_label("Select all 236 in folder").click();
    h.run();
    h.state_mut().state.flag_target();
    h.run();
    assert!(h.query_by_label("Fetching 236…").is_some());
    let fetch = h
        .state_mut()
        .state
        .take_work()
        .into_iter()
        .find_map(|w| match w {
            Work::FetchIds { ticket, .. } => Some(ticket),
            _ => None,
        });
    let ticket = fetch.expect("the ids are fetched first");
    h.key_press(egui::Key::Escape);
    h.run();
    assert!(h.state().seen.contains(&UiEvent::CancelFetch));
    let mail = &h.state().state.account("home").unwrap().mail;
    assert!(mail.fetching.is_none());
    assert!(
        mail.everything,
        "Escape went to the fetch, not the selection"
    );
    assert_eq!(mail.table.selection.len(), 6);
    done(
        &mut h.state_mut().state,
        "home",
        Done::Ids(
            ticket,
            Ok(jmap::IdPage {
                ids: vec!["L1".into()],
                total: 236,
            }),
        ),
    );
    h.run();
    assert!(
        h.state_mut().state.take_work().is_empty(),
        "nothing written"
    );
    assert!(h.query_by_label("Applies to 236 selected").is_some());
}

/// The target line with three rows selected: "Applies to 3 selected".
#[test]
fn target_line_selected_pro_light() {
    snapshot(
        "target_line_selected_pro_light",
        three_selected(),
        &theme(Scheme::Pro, Mode::Light),
    );
}

/// The target line with one row: "Applies to this email".
#[test]
fn target_line_this_email_studio_dark() {
    snapshot(
        "target_line_this_email_studio_dark",
        long_list(),
        &theme(Scheme::Studio, Mode::Dark),
    );
}

/// Deleting three selected asks first, with the count, whatever
/// `confirm_delete` says.
#[test]
fn confirm_delete_many_pro_light() {
    let mut state = three_selected();
    let mut settings = state.settings.clone();
    settings.mail.confirm_delete = false;
    state.set_settings(settings);
    state.accounts[0].mail.mailboxes.push(jmap::Mailbox {
        id: "m-trash".into(),
        name: "Trash".into(),
        role: Some("trash".into()),
        ..jmap::Mailbox::default()
    });
    state.delete_target();
    assert!(matches!(
        &state.form,
        Some(email::state::Form::Delete { ids, .. }) if ids.len() == 3
    ));
    snapshot(
        "confirm_delete_many_pro_light",
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
