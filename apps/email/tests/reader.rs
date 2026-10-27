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
use egui_kittest::kittest::{NodeT, Queryable};
use email::secrets::Secrets;
use email::shell::{Done, Event};
use email::state::{ComposeField, Mode as Surface, SessionInfo, State, Tag, UiState};
use email::view::{UiEvent, view};
use email::work::Work;
use email::{actions, label, strings};
use jmap::EmailPage;
use settings::email::{AccountEntry, Email as EmailSettings};
use toolkit::Theme;
use toolkit::action::Origin;

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
            max_size_upload: None,
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
    reader_harness(state, theme, true)
}

/// Disabling preparation exercises the frame between a state transition and
/// the next preview refresh, including the view's independent owner check.
fn reader_harness(
    state: State,
    theme: &Theme,
    prepare: bool,
) -> Harness<'static, (State, Vec<UiEvent>)> {
    // SAFETY: every test sets the same value before any date is drawn.
    unsafe { std::env::set_var("TZ", "UTC") };
    let registry = actions::registry();
    let strings = strings();
    let mut h = Harness::builder()
        .with_size(egui::vec2(620.0, 980.0))
        .build_ui_state(
            move |ui, (state, seen): &mut (State, Vec<UiEvent>)| {
                if prepare {
                    state.prepare_previews(ui.ctx());
                }
                let mut events = Vec::new();
                egui::CentralPanel::default()
                    .frame(toolkit::bars::dock_frame(ui.ctx()))
                    .show(ui, |ui| {
                        email::view::mail::panel(
                            ui,
                            state,
                            state.current().unwrap(),
                            &registry,
                            &strings,
                            &mut events,
                        );
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
    let registry = actions::registry();
    let strings = strings();
    let window_theme = theme.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1180.0, 760.0))
        .build_ui_state(
            move |ui, (state, seen): &mut (State, Vec<UiEvent>)| {
                let palette = editor::Palette::from_theme(if state.settings.composer.light {
                    toolkit::content::light_theme()
                } else {
                    &window_theme
                });
                let fired = actions::shortcuts(ui.ctx(), state, &registry);
                let mut events = view(ui, state, &registry, &strings, &palette);
                events.extend(
                    fired
                        .into_iter()
                        .map(|id| UiEvent::Command(id, toolkit::action::Origin::Shortcut)),
                );
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
        serde_json::json!({ "format": "markdown", "auto": true,
            "attachments_expanded": false, "attachments_inline": true, "attachments": [] })
    );
    assert_eq!(
        actions::registry().checked("view.reader.markdown", &h.state().0),
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
    let registry = actions::registry();
    // The menu item and `email.execute` run the same command.
    registry
        .execute("view.reader.markdown", &mut h.state_mut().0, Origin::Bus)
        .unwrap();
    h.run();
    assert_eq!(
        reader_info(&h.state().0),
        serde_json::json!({ "format": "plain", "auto": false,
            "attachments_expanded": false, "attachments_inline": true, "attachments": [] })
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
        serde_json::json!({ "format": "markdown", "auto": true,
            "attachments_expanded": false, "attachments_inline": true, "attachments": [] })
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
        serde_json::json!({ "format": "plain", "auto": true,
            "attachments_expanded": false, "attachments_inline": true, "attachments": [] })
    );
}

#[test]
fn an_html_text_body_is_never_markdown() {
    let state = opened("text/html", "<h1>Hi</h1>\n\n# not markdown\n\n## either\n");
    assert_eq!(reader_info(&state)["format"], "plain");
    let registry = actions::registry();
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
    let registry = actions::registry();
    let mut state = opened("text/plain", PLAIN);
    registry
        .execute("mail.reply_quoted", &mut state, Origin::Bus)
        .unwrap();
    let body = state.compose.take().unwrap().body.contents();
    assert!(body.contains("Bob wrote:"), "{body}");
    assert!(body.contains("\n> Lunch on Thursday works."), "{body}");
    assert!(
        body.contains("\n>> Are we still on for lunch this week?"),
        "{body}"
    );
    registry
        .execute("mail.reply_unquoted", &mut state, Origin::Bus)
        .unwrap();
    let body = state.compose.take().unwrap().body.contents();
    assert!(!body.contains("wrote:"), "{body}");
}

#[test]
fn mark_unread_keeps_the_email_open_and_unread() {
    let registry = actions::registry();
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
    let registry = actions::registry();
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
    use toolkit::action::Scope;
    let registry = actions::registry();
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

#[test]
fn composer_body_types_undoes_and_keeps_registry_chords() {
    let mut state = opened("text/plain", PLAIN);
    state.compose();
    let mut h = window(state, &theme(Scheme::Pro, Mode::Dark));
    h.get_by_label("Body").click();
    h.run();
    assert!(email::view::compose::body_focused(&h.ctx));
    h.get_by_label("Body").type_text("Typed body");
    h.run();
    assert_eq!(
        h.state().0.compose.as_ref().unwrap().body.contents(),
        "Typed body"
    );
    assert!(h.state().0.compose.as_ref().unwrap().draft.body.is_empty());
    assert!(
        !h.state()
            .1
            .iter()
            .any(|e| matches!(e, UiEvent::ComposeField(ComposeField::Body, _)))
    );
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run();
    assert_eq!(h.state().0.compose.as_ref().unwrap().body.contents(), "");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Y);
    h.run();
    assert_eq!(
        h.state().0.compose.as_ref().unwrap().body.contents(),
        "Typed body"
    );
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run();
    h.key_press_modifiers(
        egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
        egui::Key::Z,
    );
    h.run();
    assert_eq!(
        h.state().0.compose.as_ref().unwrap().body.contents(),
        "Typed body"
    );

    let registry = actions::registry();
    h.state_mut().1.clear();
    // The compose dialog's own commands (Attach files…, Ctrl+Shift+A) are
    // meant to work while the body has the keyboard, as in Thunderbird.
    for command in registry
        .iter()
        .filter(|c| c.menu != Some("compose-attachments-menu"))
    {
        for chord in registry.shortcuts_of(command.id) {
            assert!(
                !matches!(chord.logical_key, egui::Key::Z | egui::Key::Y),
                "undo belongs to the editor"
            );
            h.key_press_modifiers(chord.modifiers, chord.logical_key);
            h.run();
            assert!(email::view::compose::body_focused(&h.ctx), "{}", command.id);
            assert!(
                !h.state()
                    .1
                    .iter()
                    .any(|e| matches!(e, UiEvent::Command(..))),
                "{} fired",
                command.id
            );
        }
    }
    // A letter key produces a key event and a text event on the platform.
    for key in [egui::Key::S, egui::Key::M, egui::Key::J] {
        h.key_press(key);
        h.run();
    }
    h.get_by_label("Body").type_text("smj");
    h.run();
    assert!(
        h.state()
            .0
            .compose
            .as_ref()
            .unwrap()
            .body
            .contents()
            .ends_with("smj")
    );
    assert!(
        !h.state()
            .1
            .iter()
            .any(|e| matches!(e, UiEvent::Command(..)))
    );
    // Ctrl+Shift+A from the body opens the chooser, which takes the focus.
    h.key_press_modifiers(
        egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
        egui::Key::A,
    );
    h.run();
    let commands: Vec<_> = h
        .state()
        .1
        .iter()
        .filter(|e| matches!(e, UiEvent::Command(..)))
        .cloned()
        .collect();
    assert_eq!(
        commands,
        [UiEvent::Command(
            "compose.attach",
            toolkit::action::Origin::Shortcut
        )]
    );
}

#[test]
fn focused_body_keeps_letter_chords_before_the_view_runs() {
    let mut state = opened("text/plain", PLAIN);
    state.compose();
    // window() runs shortcuts before view(), exactly as Shell::ui does.
    let mut h = window(state, &theme(Scheme::Pro, Mode::Dark));
    h.get_by_label("Body").click();
    h.run();
    h.state_mut().1.clear();
    // Put the key and text in the same platform frame, not separate frames.
    for (key, text) in [
        (egui::Key::S, "s"),
        (egui::Key::M, "m"),
        (egui::Key::J, "j"),
    ] {
        h.input_mut().events.extend([
            egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Text(text.into()),
        ]);
        h.run();
    }
    assert_eq!(h.state().0.compose.as_ref().unwrap().body.contents(), "smj");
    assert!(
        !h.state()
            .1
            .iter()
            .any(|e| matches!(e, UiEvent::Command(..)))
    );
    assert!(email::view::compose::body_focused(&h.ctx));
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
    let registry = actions::registry();
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
    let registry = actions::registry();
    // The command toggles the remembered bar expansion.
    registry
        .execute("mail.attachments", &mut state, Origin::Bus)
        .unwrap();
    assert!(state.ui.attachments_expanded);
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
    state.ui.attachments_expanded = true;
    state.accounts[0].mail.transfers.insert(
        "2".into(),
        email::state::Transfer::Saved("/home/user/Downloads/plan.pdf".into()),
    );
    let mut h = harness(state, &theme(Scheme::Studio, Mode::Dark));
    h.run();
    h.snapshot("reader_attachments_studio_dark");
}

#[test]
fn attachment_choices_default_toggle_and_persist() {
    let mut state = with_attachments(opened("text/plain", PLAIN));
    let registry = actions::registry();
    assert_eq!(
        registry.checked("view.attachments_inline", &state),
        Some(true)
    );
    assert_eq!(registry.checked("mail.attachments", &state), Some(false));
    for id in ["mail.attachments", "view.attachments_inline"] {
        registry.execute(id, &mut state, Origin::Bus).unwrap();
    }
    assert_eq!(
        registry.checked("view.attachments_inline", &state),
        Some(false)
    );
    assert_eq!(registry.checked("mail.attachments", &state), Some(true));
    assert_eq!(reader_info(&state)["attachments_expanded"], true);
    assert_eq!(reader_info(&state)["attachments_inline"], false);
    let config = email::config::EmailConfig::from(&state);
    let dir = tempfile::tempdir().unwrap();
    let (_, file) = email::config::ConfigFile::load(dir.path());
    assert!(file.save(&config).unwrap());
    let (restored, _, _) = email::shell::start(Some(dir.path()));
    assert!(restored.ui.attachments_expanded);
    assert!(!restored.ui.attachments_inline);
    state.accounts[0].mail.open.as_mut().unwrap().id = "another-email".into();
    assert!(state.accounts[0].opening_another());
    assert!(state.ui.attachments_expanded);
    registry
        .execute("mail.attachments", &mut state, Origin::Bus)
        .unwrap();
    registry
        .execute("view.attachments_inline", &mut state, Origin::Bus)
        .unwrap();
    assert!(!state.ui.attachments_expanded);
    assert!(state.ui.attachments_inline);
}

#[test]
fn reader_attachments_collapsed_studio_dark() {
    let mut h = harness(
        with_attachments(opened("text/plain", PLAIN)),
        &theme(Scheme::Studio, Mode::Dark),
    );
    assert!(h.query_by_label("3 attachments").is_some());
    assert!(h.query_by_label("winmail.dat").is_none());
    assert!(h.get_by_label(&label("save-all")).rect().top() > 850.0);
    h.get_by_label(&label("attachments")).click();
    h.run();
    assert!(
        h.state()
            .1
            .iter()
            .any(|e| matches!(e, UiEvent::Command("mail.attachments", Origin::Ui)))
    );
    h.snapshot("reader_attachments_collapsed_studio_dark");
}

#[test]
fn expanded_tiles_open_safe_files_and_save_scripts() {
    let mut state = with_attachments(opened("text/plain", PLAIN));
    state.ui.attachments_expanded = true;
    let mut h = harness(state, &theme(Scheme::Studio, Mode::Dark));
    assert!(h.query_by_label("winmail.dat").is_some());
    assert!(h.query_by_label(&label("attachment-undecodable")).is_some());
    let first = h
        .get_by_label("Quarterly build cluster capacity plan (final, revised).pdf")
        .rect();
    let second = h.get_by_label("install.sh").rect();
    let third = h.get_by_label("winmail.dat").rect();
    assert_eq!(first.top(), second.top());
    assert!(second.left() > first.right());
    assert!(third.top() > first.bottom());
    let save = h.get_by_label(&label("save-all")).rect();
    let more = h.get_by_label(&label("attachment-actions")).rect();
    assert!(more.left() >= save.right());
    h.get_by_label("Quarterly build cluster capacity plan (final, revised).pdf")
        .click();
    h.run();
    assert!(
        h.state()
            .1
            .iter()
            .any(|e| matches!(e, UiEvent::OpenAttachment(id) if id == "2"))
    );
    h.get_by_label("install.sh").click();
    h.run();
    assert!(
        h.state()
            .1
            .iter()
            .any(|e| matches!(e, UiEvent::SaveAttachment(id) if id == "3"))
    );
}

#[test]
fn open_all_excludes_scripts_and_undecodable_parts() {
    let mut h = harness(
        with_attachments(opened("text/plain", PLAIN)),
        &theme(Scheme::Studio, Mode::Dark),
    );
    h.get_by_label(&label("attachment-actions")).click();
    h.run();
    h.get_by_label(&label("open-all")).click();
    h.run();
    let parts_opened: Vec<_> = h
        .state()
        .1
        .iter()
        .filter_map(|e| match e {
            UiEvent::OpenAttachment(part) => Some(part.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(parts_opened, ["2"]);
    let mut state = with_attachments(opened("text/plain", PLAIN));
    state.accounts[0]
        .mail
        .open
        .as_mut()
        .unwrap()
        .attachments
        .as_mut()
        .unwrap()
        .remove(0);
    let mut h = harness(state, &theme(Scheme::Studio, Mode::Dark));
    h.get_by_label(&label("attachment-actions")).click();
    h.run();
    assert!(
        h.get_by_label(&label("open-all"))
            .accesskit_node()
            .is_disabled()
    );
}

#[test]
fn attachment_bar_stays_pinned_while_the_body_scrolls() {
    let text = (0..120).map(|i| format!("Line {i}\n")).collect::<String>();
    let mut h = harness(
        with_attachments(opened("text/plain", &text)),
        &theme(Scheme::Studio, Mode::Dark),
    );
    let before = h.get_by_label(&label("save-all")).rect();
    let body_before = h.get_by_label(&text).rect();
    email::view::mail::focus_reader(&h.ctx);
    h.key_press(egui::Key::PageDown);
    h.run();
    assert!(h.get_by_label(&text).rect().top() < body_before.top());
    assert_eq!(h.get_by_label(&label("save-all")).rect(), before);
    h.key_press(egui::Key::PageUp);
    h.run();
    assert_eq!(h.get_by_label(&text).rect(), body_before);
    h.key_press(egui::Key::ArrowDown);
    h.run();
    assert!(h.get_by_label(&text).rect().top() < body_before.top());
    h.key_press(egui::Key::ArrowUp);
    h.run();
    assert_eq!(h.get_by_label(&text).rect(), body_before);
    assert_eq!(h.get_by_label(&label("save-all")).rect(), before);
}

#[test]
fn keyboard_scrolling_without_attachments() {
    let text = (0..120).map(|i| format!("Line {i}\n")).collect::<String>();
    let mut h = harness(
        opened("text/plain", &text),
        &theme(Scheme::Studio, Mode::Dark),
    );
    let before = h.get_by_label(&text).rect();
    email::view::mail::focus_reader(&h.ctx);
    h.key_press(egui::Key::PageDown);
    h.run();
    assert!(h.get_by_label(&text).rect().top() < before.top());
    h.key_press(egui::Key::PageUp);
    h.run();
    assert_eq!(h.get_by_label(&text).rect(), before);
    assert!(h.query_by_label(&label("save-all")).is_none());
}

fn with_image(mut state: State) -> State {
    let mut email = state.accounts[0].mail.open.clone().unwrap();
    email.attachments = Some(
        serde_json::from_value(serde_json::json!([
            { "partId": "2", "blobId": "B2", "size": 100, "name": "preview.png", "type": "image/png" }
        ])).unwrap(),
    );
    let ticket = request_open(&mut state, &email);
    done(
        &mut state,
        "home",
        Done::Opened(ticket, None, Box::new(Ok(email))),
    );
    state
}

fn request_open(state: &mut State, email: &jmap::Email) -> u64 {
    if !state.accounts[0]
        .mail
        .emails
        .iter()
        .any(|row| row.id == email.id)
    {
        state.accounts[0].mail.emails.push(email.clone());
    }
    assert!(state.open_email("home", email.id.clone()));
    state
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            Work::OpenEmail { ticket, .. } => Some(ticket),
            _ => None,
        })
        .expect("an open is issued")
}

/// A PNG built here rather than an external asset, delivered through the
/// real completion event and uploaded before the next UI frame.
fn finish_preview(h: &mut Harness<'static, (State, Vec<UiEvent>)>) {
    use image::ImageEncoder;
    let ticket = h
        .state_mut()
        .0
        .take_work()
        .into_iter()
        .find_map(|w| match w {
            Work::Preview { ticket, .. } => Some(ticket),
            _ => None,
        })
        .expect("preview queued");
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            &[40, 140, 200, 255].repeat(64 * 32),
            64,
            32,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    let image = email::previews::decode(&png, image::ImageFormat::Png).unwrap();
    h.state_mut().0.apply(Event::Preview(ticket, Ok(image)));
    let ctx = h.ctx.clone();
    h.state_mut().0.previews.upload(&ctx);
    h.run();
}

#[test]
fn switching_emails_preserves_previews_until_the_new_email_is_installed() {
    let mut h = reader_harness(
        with_image(opened("text/plain", PLAIN)),
        &theme(Scheme::Studio, Mode::Dark),
        false,
    );
    finish_preview(&mut h);
    let image = email::label_with("preview-image", &[("name", "preview.png")]);
    let first_texture = match h.state().0.previews.get("B2") {
        Some(email::previews::Preview::Shown(texture)) => texture.id(),
        _ => panic!("first image is shown"),
    };
    let mut second = h.state().0.accounts[0].mail.open.clone().unwrap();
    second.id = "E2".into();
    second.attachments.as_mut().unwrap()[0].blob_id = Some("B3".into());
    assert_eq!(
        second.attachments.as_ref().unwrap()[0].part_id.as_deref(),
        Some("2")
    );
    let ticket = request_open(&mut h.state_mut().0, &second);
    let ctx = h.ctx.clone();
    h.state_mut().0.prepare_previews(&ctx);
    assert!(h.state().0.accounts[0].opening_another());
    assert!(matches!(
        h.state().0.previews.get("B2"),
        Some(email::previews::Preview::Shown(texture)) if texture.id() == first_texture
    ));
    assert!(
        h.state_mut().0.take_work().is_empty(),
        "no preview is re-downloaded while B loads"
    );
    h.run();
    assert!(
        h.query_by_label(&image).is_some(),
        "A stays visible while B loads"
    );

    done(
        &mut h.state_mut().0,
        "home",
        Done::Opened(ticket, None, Box::new(Ok(second))),
    );
    // Inspect and draw before prepare_previews: the installation itself must
    // reconcile ownership, without waiting for another window update.
    assert!(h.state().0.previews.get("B2").is_none());
    assert!(matches!(
        h.state().0.previews.get("B3"),
        Some(email::previews::Preview::Loading)
    ));
    h.run();
    assert!(
        h.query_by_label(&image).is_none(),
        "B must never draw A's texture"
    );
    let loading = email::label_with("preview-loading", &[("name", "preview.png")]);
    assert!(h.query_by_label(&loading).is_some());
    finish_preview(&mut h);
    assert!(h.query_by_label(&image).is_some());
    assert!(matches!(
        h.state().0.previews.get("B3"),
        Some(email::previews::Preview::Shown(texture)) if texture.id() != first_texture
    ));
}

#[test]
fn inline_view_rejects_a_cache_owned_by_another_email() {
    let mut h = reader_harness(
        with_image(opened("text/plain", PLAIN)),
        &theme(Scheme::Studio, Mode::Dark),
        false,
    );
    finish_preview(&mut h);
    // Even a shared blob must not make the view trust a stale cache owner.
    h.state_mut().0.accounts[0].mail.open.as_mut().unwrap().id = "E2".into();
    h.run();
    assert!(matches!(
        h.state().0.previews.get("B2"),
        Some(email::previews::Preview::Shown(_))
    ));
    let image = email::label_with("preview-image", &[("name", "preview.png")]);
    assert!(h.query_by_label(&image).is_none());
    assert_eq!(
        reader_info(&h.state().0)["attachments"][0]["preview"]["state"],
        "none"
    );
}

#[test]
fn reopening_an_email_replaces_previews_only_when_installed() {
    let mut h = reader_harness(
        with_image(opened("text/plain", PLAIN)),
        &theme(Scheme::Studio, Mode::Dark),
        false,
    );
    finish_preview(&mut h);
    let email = h.state().0.accounts[0].mail.open.clone().unwrap();
    let ticket = request_open(&mut h.state_mut().0, &email);
    let ctx = h.ctx.clone();
    h.state_mut().0.prepare_previews(&ctx);
    assert!(matches!(
        h.state().0.previews.get("B2"),
        Some(email::previews::Preview::Shown(_))
    ));
    assert!(h.state_mut().0.take_work().is_empty());
    done(
        &mut h.state_mut().0,
        "home",
        Done::Opened(ticket, None, Box::new(Ok(email))),
    );
    assert!(matches!(
        h.state().0.previews.get("B2"),
        Some(email::previews::Preview::Loading)
    ));
    assert_eq!(h.state().0.accounts[0].mail.open_ticket, ticket);
}

#[test]
fn mailbox_changes_clear_previews_before_the_next_frame() {
    for refresh in [false, true] {
        let mut state = with_image(opened("text/plain", PLAIN));
        assert!(state.previews.get("B2").is_some());
        let mailbox = jmap::Mailbox {
            id: "m-other".into(),
            name: "Other".into(),
            ..jmap::Mailbox::default()
        };
        if refresh {
            // The selected mailbox disappeared from the server's list.
            done(&mut state, "home", Done::Mailboxes(Ok(vec![mailbox])));
        } else {
            state.accounts[0].mail.mailboxes.push(mailbox);
            assert!(state.select_mailbox("home", "m-other".into()));
        }
        assert!(state.accounts[0].mail.open.is_none());
        assert!(state.previews.get("B2").is_none());
    }
}

#[test]
fn reconnecting_clears_previews_before_the_next_frame() {
    let mut state = with_image(opened("text/plain", PLAIN));
    assert!(state.previews.get("B2").is_some());
    assert!(state.reconnect("home"));
    assert!(state.accounts[0].mail.open.is_none());
    assert!(state.previews.get("B2").is_none());
}

#[test]
fn reader_inline_image_studio_dark() {
    let mut h = harness(
        with_image(opened(
            "text/markdown",
            "## A message\n\nHere is **the image**.",
        )),
        &theme(Scheme::Studio, Mode::Dark),
    );
    let loading = email::label_with("preview-loading", &[("name", "preview.png")]);
    assert!(h.query_by_label(&loading).is_some());
    assert!(h.query_by_label("1 attachment").is_some());
    assert_eq!(
        reader_info(&h.state().0)["attachments"][0]["preview"]["state"],
        "loading"
    );
    finish_preview(&mut h);
    let image = email::label_with("preview-image", &[("name", "preview.png")]);
    let rect = h.get_by_label(&image).rect();
    assert!(rect.width() * h.ctx.pixels_per_point() <= 64.01);
    assert!((rect.width() / rect.height() - 2.0).abs() < 0.01);
    assert!(h.query_by_label(&loading).is_none());
    assert_eq!(
        reader_info(&h.state().0)["attachments"][0]["preview"]["state"],
        "shown"
    );
    h.get_by_label(&image).click();
    h.run();
    assert!(
        h.state()
            .1
            .iter()
            .any(|e| matches!(e, UiEvent::OpenAttachment(id) if id == "2"))
    );
    h.snapshot("reader_inline_image_studio_dark");
    // The same cached image follows the source view inside the body scroller.
    actions::registry()
        .execute("view.reader.markdown", &mut h.state_mut().0, Origin::Bus)
        .unwrap();
    h.run();
    assert!(h.query_by_label(&image).is_some());
    assert_eq!(reader_info(&h.state().0)["format"], "plain");
}

#[test]
fn reader_inline_off_studio_dark() {
    let mut h = harness(
        with_image(opened("text/plain", PLAIN)),
        &theme(Scheme::Studio, Mode::Dark),
    );
    finish_preview(&mut h);
    actions::registry()
        .execute("view.attachments_inline", &mut h.state_mut().0, Origin::Bus)
        .unwrap();
    h.run();
    let image = email::label_with("preview-image", &[("name", "preview.png")]);
    assert!(h.query_by_label(&image).is_none());
    assert!(h.query_by_label("preview.png").is_none());
    assert!(h.query_by_label("1 attachment").is_some());
    assert!(h.state().0.previews.get("B2").is_none(), "texture freed");
    assert_eq!(
        reader_info(&h.state().0)["attachments"][0]["preview"]["state"],
        "none"
    );
    h.snapshot("reader_inline_off_studio_dark");
}

#[test]
fn inline_image_context_menu_saves_and_failed_previews_keep_the_bar() {
    let mut h = harness(
        with_image(opened("text/plain", PLAIN)),
        &theme(Scheme::Studio, Mode::Dark),
    );
    finish_preview(&mut h);
    let image = email::label_with("preview-image", &[("name", "preview.png")]);
    let at = h.get_by_label(&image).rect().center();
    h.hover_at(at);
    h.run();
    // The toolbar always has Reply All; the reader's context menu would add
    // a second one.
    let reply_alls = h.query_all_by_label(&label("reply-all")).count();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    h.run();
    assert!(h.query_by_label(&label("open")).is_some());
    assert_eq!(
        h.query_all_by_label(&label("reply-all")).count(),
        reply_alls,
        "the reader's context menu opened over the image's"
    );
    h.get_by_label(&label("save")).click();
    h.run();
    assert!(
        h.state()
            .1
            .iter()
            .any(|e| matches!(e, UiEvent::SaveAttachment(id) if id == "2"))
    );

    let mut h = harness(
        with_image(opened("text/plain", PLAIN)),
        &theme(Scheme::Studio, Mode::Dark),
    );
    let ticket = h
        .state_mut()
        .0
        .take_work()
        .into_iter()
        .find_map(|w| match w {
            Work::Preview { ticket, .. } => Some(ticket),
            _ => None,
        })
        .unwrap();
    h.state_mut()
        .0
        .apply(Event::Preview(ticket, Err("invalid image".into())));
    h.run();
    let failure = email::label_with(
        "preview-failed",
        &[("name", "preview.png"), ("reason", "invalid image")],
    );
    assert!(h.query_by_label(&failure).is_some());
    assert!(h.query_by_label("1 attachment").is_some());
    assert_eq!(
        reader_info(&h.state().0)["attachments"][0]["preview"],
        serde_json::json!({"state":"failed", "reason":"invalid image"})
    );
}

#[test]
fn hovering_an_inline_image_keeps_the_reader_context_menu_open() {
    let mut h = harness(
        with_image(opened("text/plain", PLAIN)),
        &theme(Scheme::Studio, Mode::Dark),
    );
    finish_preview(&mut h);
    let image = email::label_with("preview-image", &[("name", "preview.png")]);
    let image_at = h.get_by_label(&image).rect().center();
    let reply_alls = h.query_all_by_label(&label("reply-all")).count();
    // Open the menu at the text's centre: the popup hangs right of that
    // point, clear of the small image at the left edge, so the hover below
    // reaches the image rather than a menu row covering it.
    let at = h.get_by_label(PLAIN).rect().center();
    assert!(at.x > h.get_by_label(&image).rect().right());
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
    h.run();
    assert_eq!(
        h.query_all_by_label(&label("reply-all")).count(),
        reply_alls + 1
    );
    h.hover_at(image_at);
    h.run();
    assert_eq!(
        h.query_all_by_label(&label("reply-all")).count(),
        reply_alls + 1,
        "hovering the image must not hide the reader's Message menu"
    );
    assert!(
        h.query_by_label(&label("open")).is_none(),
        "no attachment menu opened"
    );
}

#[test]
fn turning_inline_off_drops_an_in_flight_result() {
    let mut h = harness(
        with_image(opened("text/plain", PLAIN)),
        &theme(Scheme::Studio, Mode::Dark),
    );
    actions::registry()
        .execute("view.attachments_inline", &mut h.state_mut().0, Origin::Bus)
        .unwrap();
    finish_preview(&mut h);
    assert!(h.state().0.previews.get("B2").is_none());
    assert!(h.query_by_label("1 attachment").is_some());
    assert_eq!(
        reader_info(&h.state().0)["attachments"][0]["preview"]["state"],
        "none"
    );
}

#[test]
fn closed_email_preview_results_never_reach_the_new_reader() {
    let mut state = with_image(opened("text/plain", PLAIN));
    state.prepare_previews(&egui::Context::default());
    let ticket = state
        .take_work()
        .into_iter()
        .find_map(|w| match w {
            Work::Preview { ticket, .. } => Some(ticket),
            _ => None,
        })
        .unwrap();
    state.accounts[0].mail.open = None;
    state.apply(Event::Preview(ticket, Err("old failure".into())));
    assert!(state.previews.get("B2").is_none());
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
    actions::registry()
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
        .memory_mut(|m| m.request_focus(egui::Id::unique("compose-recipients").with("to")));
    h.run();
    h.snapshot("compose_suggestions_studio_dark");
}

/// The compose field named `name`. A row's label names its field (egui
/// folds a `labelled_by` label into the field's accessible name), so the
/// field is how a row is found.
fn field(h: &Harness<'static, (State, Vec<UiEvent>)>, name: &str) -> Option<egui::Rect> {
    use egui_kittest::kittest::NodeT;
    h.query_all_by_label(name)
        .find(|n| n.accesskit_node().role() == egui::accesskit::Role::TextInput)
        .map(|n| n.rect())
}

fn button(h: &Harness<'static, (State, Vec<UiEvent>)>, key: &str) -> egui::Rect {
    h.get_by_label(&label(key)).rect()
}

#[test]
fn composer_uses_taller_padded_fields_without_losing_shared_editor_input() {
    let mut overlay = toolkit::theme::Overlay::default();
    for (name, value) in [("field_height", 48.0), ("field_padding_y", 8.0)] {
        overlay
            .style
            .insert(name.into(), design::StyleValueSource::Number(value));
    }
    let theme = theme(Scheme::Studio, Mode::Dark)
        .with_overlay(overlay)
        .unwrap();
    let mut state = opened("text/plain", PLAIN);
    assert!(state.open_compose(Some("home"), Default::default()));
    let mut h = window(state, &theme);
    h.run();
    let to = field(&h, "To").unwrap();
    let subject = field(&h, "Subject").unwrap();
    let cc = button(&h, "show-cc");
    assert!((cc.height() - 48.0).abs() < 1.0, "{cc:?}");
    assert!((cc.center().y - to.center().y).abs() < 4.0);
    assert!(subject.top() > to.bottom(), "padded rows do not overlap");
    h.get_by_label("Body").click();
    h.get_by_label("Body").type_text("Body with taller fields");
    h.run();
    assert_eq!(
        h.state().0.compose.as_ref().unwrap().body.contents(),
        "Body with taller fields"
    );
}

#[test]
fn compose_rows_put_labels_outside_and_open_cc_bcc_and_reply_to_from_the_to_row() {
    let mut state = opened("text/plain", PLAIN);
    assert!(state.open_compose(Some("home"), Default::default()));
    let mut h = window(state, &theme(Scheme::Studio, Mode::Dark));
    h.run();
    let to = field(&h, "To").expect("To");
    let subject = field(&h, "Subject").expect("Subject");
    assert!(subject.top() > to.bottom(), "Subject under To");
    // Cc, Bcc and Reply-To are buttons at the right of the To row, their
    // rows hidden until asked for.
    for (row, tip) in [
        ("Cc", "show-cc"),
        ("Bcc", "show-bcc"),
        ("Reply-To", "show-reply-to"),
    ] {
        assert!(field(&h, row).is_none(), "{row} hidden at first");
        let b = button(&h, tip);
        assert!(
            (b.center().y - to.center().y).abs() < 4.0,
            "{tip} on the To row"
        );
        assert!(b.left() > to.right(), "{tip} right of the field");
    }
    // Identities… is on the row above To (From), ending where the To row's
    // buttons end.
    let identities = button(&h, "identities-open");
    assert!(identities.bottom() <= to.top() + 1.0);
    let last = button(&h, "show-reply-to");
    assert!(
        (identities.right() - last.right()).abs() < 2.0,
        "{identities:?} {last:?}"
    );
    // Each button opens its row under To.
    for tip in ["show-cc", "show-bcc", "show-reply-to"] {
        h.get_by_label(&label(tip)).click();
        h.run();
        h.run();
    }
    let cc = field(&h, "Cc").expect("Cc opened");
    let reply_to = field(&h, "Reply-To").expect("Reply-To opened");
    let subject = field(&h, "Subject").unwrap();
    let to = field(&h, "To").unwrap();
    assert!(to.bottom() <= cc.top() && reply_to.bottom() <= subject.top());
    // Labels outside, in one column: every field starts at the same x,
    // well in from the dialog's edge.
    for (name, rect) in [("Cc", cc), ("Reply-To", reply_to), ("Subject", subject)] {
        assert!(
            (rect.left() - to.left()).abs() < 1.0,
            "{name} lines up with To"
        );
    }
    // A Reply-To typed here is the draft's, and the button closes an
    // empty row again.
    h.state_mut().0.apply_ui(UiEvent::ComposeField(
        email::state::ComposeField::ReplyTo,
        "help@home.example.invalid".into(),
    ));
    h.run();
    h.snapshot("compose_rows_studio_dark");
    let compose = h.state().0.compose.clone().unwrap();
    assert_eq!(
        compose.draft.reply_to.as_deref(),
        Some("help@home.example.invalid")
    );
    assert!(compose.keepable());
    h.get_by_label(&label("show-cc")).click();
    h.run();
    h.run();
    assert!(field(&h, "Cc").is_none(), "empty Cc closes");
    h.get_by_label(&label("show-reply-to")).click();
    h.run();
    h.run();
    assert!(field(&h, "Reply-To").is_some(), "a filled row stays");
}

#[test]
fn compose_sender_and_recipient_controls_have_fluent_accessible_names() {
    use egui_kittest::kittest::NodeT;
    let mut state = opened("text/plain", PLAIN);
    let entry = &mut state.accounts[0].entry;
    let first = entry.senders()[0].clone();
    let mut second = first.clone();
    second.id = "other".into();
    second.address = "other@example.invalid".into();
    entry.identities = vec![first, second];
    assert!(state.open_compose(Some("home"), Default::default()));
    let mut h = window(state, &theme(Scheme::Studio, Mode::Dark));
    h.run();
    assert!(
        h.query_all_by_label(&label("from"))
            .any(|node| node.accesskit_node().role() == egui::accesskit::Role::ComboBox)
    );
    for tip in ["show-cc", "show-bcc"] {
        h.get_by_label(&label(tip)).click();
        h.run();
    }
    for key in ["to", "cc", "bcc"] {
        assert!(
            field(&h, &label(key)).is_some(),
            "{key} editor has its Fluent name"
        );
    }
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
    state.settings.reader.light = true;
    let mut h = harness(state, &theme(Scheme::Studio, Mode::Dark));
    h.run();
    h.snapshot("reader_light_content_studio_dark");
}

#[test]
fn reader_light_is_saved_as_the_reader_style() {
    // The toggle is a profile change: the writer turns the reader's style
    // into the one `email.reader` write, and nothing else moves.
    let before = settings::email::Email::default();
    let light = settings::content::Content {
        light: true,
        ..before.reader.clone()
    };
    let (next, _) = email::writer::apply(
        &before,
        &email::writer::Change::Surface(email::writer::Surface::Reader, light),
    )
    .expect("the reader style is always writable");
    assert!(next.reader.light);
    assert_eq!(next.composer, before.composer);
    let changed = email::writer::changes(&before, &next);
    assert_eq!(changed["email.reader"]["light"], true);
    assert_eq!(changed.as_object().map(|m| m.len()), Some(1));
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
    actions::registry()
        .execute("mail.reply_quoted", &mut state, Origin::Bus)
        .unwrap();
    let mut h = window(state, &theme(Scheme::Studio, Mode::Dark));
    h.run();
    let send = h.get_by_label(&label("send")).rect();
    let cancel = h.get_by_label(&label("cancel")).rect();
    let toggle = h.get_by_label(&label("reply-below-quote")).rect();
    let screen = h.ctx.content_rect();
    for rect in [send, cancel, toggle] {
        assert!(
            screen.contains_rect(rect),
            "composer footer stays on screen: {rect:?}"
        );
    }
    assert!(toggle.bottom() < send.top());
    h.snapshot("quoted_reply_compose_studio_dark");
}
