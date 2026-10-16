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
use email::secrets::Secrets;
use email::shell::{Done, Event};
use email::state::{Mode as Surface, Node, Phase, SessionInfo, State, Tag, UiState};
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
    harness_sized(state, theme, egui::vec2(1180.0, 760.0))
}

fn harness_sized(state: State, theme: &Theme, size: egui::Vec2) -> Harness<'static, Window> {
    // Dates render in UTC, whatever the worker's zone.
    // SAFETY: every test sets the same value before any date is drawn.
    unsafe { std::env::set_var("TZ", "UTC") };
    let registry = commands::registry();
    let strings = strings();
    let mut h = Harness::builder()
        .with_size(size)
        .with_step_dt(0.05)
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
            serde_json::json!(
                "cy.no-display-name.but-a-very-long-local-part@subdomain.example.invalid"
            ),
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

#[test]
fn an_unread_email_row_opens_and_marks_read() {
    let mut h = harness(fixture());
    h.get_by_label_contains("Bob").click();
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

/// The message list's rows, top to bottom, by their accessible labels.
fn list_rows(h: &Harness<'static, Window>) -> Vec<String> {
    use egui_kittest::kittest::NodeT;
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
fn each_email_is_one_row_of_from_subject_and_a_compact_date() {
    let h = harness(long_list());
    assert_eq!(
        list_rows(&h),
        [
            "Ann Lee, Lunch today?, 08:05",
            "Bartholomew Rivera-Montgomery of the Extremely Long Display Name Society, Quarterly report: the full numbers, every appendix, the board's comments on each, and a summary of the summary, Wed 14:02",
            "cy.no-display-name.but-a-very-long-local-part@subdomain.example.invalid, Minutes, Mon 17:45",
            "Dee, Re: build cluster, 3 Aug",
            "Eve, Photos from the weekend, 16 Jan",
            "Gus, Old thread, 16/10/2025",
        ]
    );
    // The pager reads the server's total.
    assert!(h.query_by_label("1–6 of 236").is_some());
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

#[test]
fn a_header_click_sorts_on_the_server() {
    let mut h = harness(fixture());
    h.get_by_label(&label("column-subject")).click();
    h.run();
    let state = &mut h.state_mut().state;
    assert!(state.mail_sorted_by(email::state::COLUMN_SUBJECT));
    let fetched = state.take_work().into_iter().any(|w| {
        matches!(w, Work::FetchEmails { position: 0, sort, .. }
            if sort == jmap::EmailSort::new(jmap::SortProperty::Subject, true))
    });
    assert!(fetched);
}

#[test]
fn the_pager_and_the_keyboard_drive_the_list() {
    let mut h = harness(long_list());
    h.get_by_label("Next page").click();
    h.run();
    assert!(h.state_mut().state.take_work().iter().any(|w| matches!(
        w,
        Work::FetchEmails {
            position: 100,
            limit: 100,
            ..
        }
    )));
    // A row takes the keyboard; Down opens the next email.
    let mut h = harness(long_list());
    h.get_by_label_contains("Minutes").click();
    h.run();
    h.state_mut().state.take_work();
    h.key_press(egui::Key::ArrowDown);
    h.run();
    assert!(
        h.state_mut()
            .state
            .take_work()
            .iter()
            .any(|w| matches!(w, Work::OpenEmail { id, .. } if id == "L4"))
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

/// The long list as a search's result: `text` matched, `shown` of its rows
/// left, in `scope`.
fn searched(text: &str, keep: &[&str], scope: email::state::Scope) -> State {
    let mut state = long_list();
    let view = &mut state.accounts[0];
    view.mail.search = email::state::Search {
        text: text.into(),
        applied: text.into(),
        scope,
    };
    view.mail.emails.retain(|e| keep.contains(&e.id.as_str()));
    view.mail.total = keep.len() as u64;
    view.mail.table.selection.clear();
    view.mail.table.focus = None;
    state
}

/// The long list searched in every folder: each email says where it is.
fn all_folders() -> State {
    let mut state = searched("re", &["L2", "L4", "L5"], email::state::Scope::All);
    let folders = ["m-in", "m-rust", "m-sent"];
    for (email, folder) in state.accounts[0].mail.emails.iter_mut().zip(folders) {
        email.mailbox_ids = Some([(folder.to_owned(), true)].into_iter().collect());
    }
    state
}

#[test]
fn search_results_studio_light() {
    snapshot(
        "search_results_studio_light",
        searched("re", &["L2", "L4"], email::state::Scope::Folder),
        &theme(Scheme::Studio, Mode::Light),
    );
}

#[test]
fn search_no_match_studio_dark() {
    snapshot(
        "search_no_match_studio_dark",
        searched("zebra", &[], email::state::Scope::Folder),
        &theme(Scheme::Studio, Mode::Dark),
    );
}

#[test]
fn search_all_folders_pro_light() {
    let h = harness(all_folders());
    // The Folder column names each email's mailbox.
    let rows = list_rows(&h);
    assert!(rows[1].contains(", Rust, "), "{rows:?}");
    drop(h);
    snapshot(
        "search_all_folders_pro_light",
        all_folders(),
        &theme(Scheme::Pro, Mode::Light),
    );
}

#[test]
fn the_header_counts_results_and_the_field_searches_after_a_pause() {
    let mut h = harness(searched("re", &["L2", "L4"], email::state::Scope::Folder));
    assert!(h.query_by_label("2 results").is_some());
    let mut h2 = harness(long_list());
    assert!(h2.query_by_label("236 messages").is_some());
    h2.get_by_role(egui::accesskit::Role::TextInput).click();
    h2.run();
    h2.get_by_role(egui::accesskit::Role::TextInput)
        .type_text("tax");
    h2.run();
    let work = h2.state_mut().state.take_work();
    assert!(
        work.iter()
            .any(|w| matches!(w, Work::SearchDue { .. })),
        "{work:?}"
    );
    assert_eq!(h2.state().state.accounts[0].mail.search.text, "tax");
    // The × clears at once.
    h2.get_by_label("Clear search").click();
    h2.run();
    assert!(h2.state().state.accounts[0].mail.search.text.is_empty());
    // Compose runs the registry's command.
    h.get_by_label(&label("compose")).click();
    h.run();
    assert!(
        h.state()
            .seen
            .iter()
            .any(|e| matches!(e, UiEvent::Command("mail.compose", _)))
    );
}

#[test]
fn compose_gives_way_to_its_icon_before_the_search_field_shrinks() {
    let narrow = harness_sized(
        long_list(),
        &theme(Scheme::Pro, Mode::Light),
        egui::vec2(820.0, 520.0),
    );
    let wide = harness(long_list());
    let compose = |h: &Harness<'static, Window>| h.get_by_label(&label("compose")).rect().width();
    assert!(compose(&narrow) < compose(&wide) / 2.0);
    let field = narrow
        .get_by_role(egui::accesskit::Role::TextInput)
        .rect()
        .width();
    assert!(field >= 80.0, "{field}");
    drop(wide);
    let mut narrow = narrow;
    narrow.run();
    narrow.snapshot("list_header_narrow_pro_light");
}

/// Right-click the tree row `label` and snapshot its open menu.
fn tree_menu(label: &str, name: &str) -> Harness<'static, Window> {
    let mut h = harness(fixture());
    h.get_by_label(label).click_secondary();
    h.run();
    h.snapshot(name);
    h
}

#[test]
fn each_tree_node_has_its_registry_menu() {
    let h = tree_menu("Lists", "tree_menu_folder_pro_light");
    for item in ["New Subfolder…", "Rename…", "Delete Folder…", "Mark Folder as Read", "Refresh"] {
        assert!(h.query_by_label(item).is_some(), "{item}");
    }
    assert!(h.query_by_label("Empty…").is_none(), "not a bin");
    let state = &h.state().state;
    assert_eq!(
        state.ui.menu_node,
        Some(Node::Mailbox("home".into(), "m-lists".into()))
    );
    assert_eq!(
        state.account("home").unwrap().mail.selected.as_deref(),
        Some("m-lists"),
        "a right-click selects"
    );
    let h = tree_menu("Home", "tree_menu_account_pro_light");
    for item in ["Compose from This Account", "Reconnect", "Account Settings…"] {
        assert!(h.query_by_label(item).is_some(), "{item}");
    }
    let mut h = tree_menu("Main", "tree_menu_calendar_pro_light");
    assert!(h.query_by_label(&label("new-event")).is_some());
    h.get_by_label(&label("new-event")).click();
    h.run();
    assert!(
        h.state()
            .seen
            .iter()
            .any(|e| matches!(e, UiEvent::Command("calendar.new_event", _)))
    );
}

#[test]
fn a_role_folder_menu_disables_rename_and_delete() {
    let mut h = harness(fixture());
    h.get_by_label("Inbox").click_secondary();
    h.run();
    let rename = h.get_by_label("Rename…");
    assert!(rename.accesskit_node().is_disabled());
    let mark = h.get_by_label("Mark Folder as Read");
    assert!(!mark.accesskit_node().is_disabled(), "the inbox has unread");
}

#[test]
fn a_row_right_click_selects_it_and_opens_the_message_menu() {
    let mut h = harness(long_list());
    h.get_by_label_contains("Photos from").click_secondary();
    h.run();
    assert!(
        h.state()
            .seen
            .iter()
            .any(|e| matches!(e, UiEvent::MailTable(_, toolkit::datatable::TableEvent::ContextMenu { key, .. }) if key == "L5"))
    );
    assert_eq!(h.state().state.accounts[0].mail.table.selection, ["L5"]);
    assert!(h.query_by_label(&label("reply")).is_some(), "the message commands");
    h.snapshot("list_menu_pro_light");
}

/// Press on the list row `row`, then move to the tree row `target`.
fn drag_to(h: &mut Harness<'static, Window>, row: &str, target: &str) -> egui::Pos2 {
    let from = h.get_by_label_contains(row).rect().center();
    let to = h.get_by_label(target).rect().center();
    h.hover_at(from);
    h.event(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    for n in 1..=4 {
        let t = n as f32 / 4.0;
        h.hover_at(from + (to - from) * t);
        h.step();
    }
    to
}

#[test]
fn a_drag_onto_a_folder_moves_the_rows_it_carries() {
    let mut h = harness(long_list());
    let to = drag_to(&mut h, "Re: build cluster", "Rust");
    // An unselected row's drag carries it alone, and selects it.
    assert_eq!(h.state().state.accounts[0].mail.table.selection, ["L4"]);
    h.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    assert!(h.state().seen.iter().any(|e| matches!(
        e,
        UiEvent::DropOnFolder(Node::Mailbox(_, mailbox), keys) if mailbox == "m-rust" && keys == &["L4".to_owned()]
    )));
    let work = h.state_mut().state.take_work();
    assert!(work.iter().any(|w| matches!(
        w,
        Work::Write { write: email::verbs::Write::Move { ids, mailbox }, .. }
            if mailbox == "m-rust" && ids == &["L4".to_owned()]
    )));
}

#[test]
fn a_drag_over_the_source_folder_is_refused() {
    let mut h = harness(long_list());
    let to = drag_to(&mut h, "Lunch today?", "Inbox");
    h.event(egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run();
    assert!(
        !h.state()
            .seen
            .iter()
            .any(|e| matches!(e, UiEvent::DropOnFolder(..)))
    );
}

#[test]
fn drag_hover_on_a_folder_pro_light() {
    let mut h = harness(long_list());
    drag_to(&mut h, "Re: build cluster", "Rust");
    h.step();
    h.snapshot("drag_hover_folder_pro_light");
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
