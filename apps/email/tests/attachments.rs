// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The attachments popup, driven through kittest with no network:
//! `mail.attachments` opens it over an email with attachments, Save and
//! Open queue a download for the right part, a program offers Save only,
//! an undecodable part neither, and offscreen snapshots of the popup.
//! Regenerate the images with `UPDATE_SNAPSHOTS=1` and look at them before
//! committing.
use design::{DesignContext, Mode, Scheme};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use email::attachments::Target;
use email::secrets::Secrets;
use email::shell::{Done, Event};
use email::state::{Dialog, Mode as Surface, SessionInfo, State, Tag, UiState};
use email::view::{UiEvent, view};
use email::work::Work;
use email::{commands, label, strings};
use jmap::EmailPage;
use settings::email::{AccountEntry, Email as EmailSettings};
use toolkit::Theme;
use toolkit::command::Origin;

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

/// One connected account with an email open that carries four
/// attachments: a PDF, a nameless image, a shell script and a part the
/// server could not decode.
fn opened() -> State {
    let entry = AccountEntry {
        id: "home".into(),
        name: "Home".into(),
        url: "https://home.example.invalid".into(),
        email: "ann@home.example.invalid".into(),
        ca_path: None,
        enabled: true,
        signature: String::new(),
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
        "subject": "Quarterly figures",
        "from": [{ "name": "Bob", "email": "bob@example.invalid" }],
        "receivedAt": "2026-10-13T01:02:03Z",
        "hasAttachment": true,
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
    full["textBody"] = serde_json::json!([{ "partId": "1", "type": "text/plain" }]);
    full["bodyValues"] = serde_json::json!({ "1": { "value": "Figures attached." } });
    full["attachments"] = serde_json::json!([
        {
            "partId": "2", "blobId": "B2", "size": 48213,
            "name": "Q3 figures — final, with the board's corrections and the appendix.pdf",
            "type": "application/pdf", "disposition": "attachment"
        },
        { "partId": "3", "blobId": "B3", "size": 1_300_000, "type": "image/png", "disposition": "inline" },
        {
            "partId": "4", "blobId": "B4", "size": 512, "name": "install.sh",
            "type": "application/x-shellscript", "disposition": "attachment"
        },
        { "partId": "5", "size": 2048, "name": "winmail.dat", "type": "application/ms-tnef", "undecodable": true }
    ]);
    done(
        &mut state,
        "home",
        Done::Opened(
            ticket,
            None,
            Box::new(Ok(serde_json::from_value(full).unwrap())),
        ),
    );
    state.take_work();
    state
}

fn theme(scheme: Scheme, mode: Mode) -> Theme {
    Theme::for_context(DesignContext {
        scheme,
        mode,
        ..DesignContext::default()
    })
}

struct Window {
    state: State,
    seen: Vec<UiEvent>,
}

/// The whole window over `state`; it applies every frame's events.
fn harness(state: State, theme: &Theme) -> Harness<'static, Window> {
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

fn popup() -> State {
    let mut state = opened();
    commands::registry()
        .execute("mail.attachments", &mut state, Origin::Ui)
        .unwrap();
    assert_eq!(state.ui.dialog, Some(Dialog::Attachments));
    state
}

fn downloads(state: &mut State) -> Vec<(String, Target, usize)> {
    state
        .take_work()
        .into_iter()
        .filter_map(|w| match w {
            Work::Attachment {
                blob,
                target,
                index,
                ..
            } => Some((blob, target, index)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_command_is_enabled_only_with_attachments() {
    let state = opened();
    let registry = commands::registry();
    let enabled = |state: &State| (registry.get("mail.attachments").unwrap().enabled)(state);
    assert!(enabled(&state));
    let mut bare = opened();
    bare.accounts[0].mail.open.as_mut().unwrap().attachments = None;
    assert!(!enabled(&bare));
}

#[test]
fn save_and_open_queue_the_part_and_a_program_saves_only() {
    let mut h = harness(popup(), &theme(Scheme::Pro, Mode::Light));
    let (save, open) = (label("save"), label("open"));
    let saves: Vec<_> = h.query_all_by_label(&save).collect();
    assert_eq!(saves.len(), 4, "a Save per row");
    saves[0].click();
    h.run();
    let queued = downloads(&mut h.state_mut().state);
    assert_eq!(queued, [("B2".to_owned(), Target::Save(None), 1)]);
    // The row under way shows no buttons; the first Open left is the
    // second row's, the nameless image.
    let opens: Vec<_> = h.query_all_by_label(&open).collect();
    assert_eq!(opens.len(), 3);
    opens[0].click();
    h.run();
    let queued = downloads(&mut h.state_mut().state);
    assert_eq!(queued, [("B3".to_owned(), Target::Open, 2)]);
    // The script's Open and the undecodable part's buttons do nothing.
    let opens: Vec<_> = h.query_all_by_label(&open).collect();
    assert_eq!(opens.len(), 2);
    opens[0].click();
    opens[1].click();
    h.run();
    let saves: Vec<_> = h.query_all_by_label(&save).collect();
    saves[1].click();
    h.run();
    assert!(downloads(&mut h.state_mut().state).is_empty());
    // Save All takes every part that can be downloaded and is not under
    // way: the script.
    h.get_by_label(&label("save-all")).click();
    h.run();
    let blobs: Vec<String> = downloads(&mut h.state_mut().state)
        .into_iter()
        .map(|(blob, _, _)| blob)
        .collect();
    assert_eq!(blobs, ["B4"], "B2 and B3 are under way");
    // Done closes the popup.
    h.get_by_label(&label("done")).click();
    h.run();
    assert_eq!(h.state().state.ui.dialog, None);
}

#[test]
fn an_answered_download_names_the_file_on_the_status_line() {
    let mut state = popup();
    state.apply_ui(UiEvent::Attachment("2".into(), false));
    let ticket = *state.attaching.keys().next().unwrap();
    state.apply(Event::Attached(
        ticket,
        None,
        false,
        Ok("/home/ann/Downloads/report.pdf".into()),
    ));
    assert!(state.attaching.is_empty());
    assert!(state.status.contains("/home/ann/Downloads/report.pdf"));
}

fn snapshot(name: &str, theme: &Theme) {
    let mut state = popup();
    // One row under way, as it shows while downloading.
    state.apply_ui(UiEvent::Attachment("3".into(), false));
    let mut h = harness(state, theme);
    h.run();
    h.snapshot(name);
}

#[test]
fn attachments_pro_light() {
    snapshot("attachments_pro_light", &theme(Scheme::Pro, Mode::Light));
}

#[test]
fn attachments_studio_dark() {
    snapshot(
        "attachments_studio_dark",
        &theme(Scheme::Studio, Mode::Dark),
    );
}
