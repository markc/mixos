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
use email::view::UiEvent;
use email::work::Work;
use email::{commands, label};
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
mix cbc-build.mix --worker cbc3 -- cargo test -p email
```

| worker | load | jobs |
|--------|-----:|-----:|
| cbc2   | 0.4  | 2    |
| cbc3   | 1.2  | 5    |

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
    let mut h = Harness::builder()
        .with_size(egui::vec2(620.0, 980.0))
        .build_ui_state(
            |ui, (state, seen): &mut (State, Vec<UiEvent>)| {
                let mut events = Vec::new();
                egui::CentralPanel::default()
                    .frame(toolkit::bars::dock_frame(ui.ctx()))
                    .show(ui, |ui| {
                        let title = label("reader");
                        toolkit::panel::group(ui, "reader", &[title.as_str()], |ui, _| {
                            email::view::mail::reader(ui, state.current().unwrap(), &mut events);
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

fn reader_info(state: &State) -> serde_json::Value {
    state.info()["accounts"][0]["mail"]["reader"].clone()
}

#[test]
fn a_markdown_written_body_renders_as_markdown() {
    let h = harness(opened("text/plain", MARKDOWN), &theme(Scheme::Pro, Mode::Light));
    for text in ["What changed", "Running it", "worker", "cbc3", "fixtures stay small"] {
        assert!(h.query_by_label(text).is_some(), "{text}");
    }
    assert!(h.query_by_label(&label("show-source")).is_some());
    // No raw syntax is on screen.
    assert!(h.query_by_label_contains("## What changed").is_none());
    assert_eq!(
        reader_info(&h.state().0),
        serde_json::json!({ "format": "markdown", "auto": true })
    );
}

#[test]
fn the_toggle_shows_the_source_and_back() {
    let mut h = harness(opened("text/plain", MARKDOWN), &theme(Scheme::Pro, Mode::Light));
    let registry = commands::registry();
    h.get_by_label(&label("show-source")).click();
    h.run();
    assert!(
        h.state()
            .1
            .contains(&UiEvent::Command("view.reader.markdown", Origin::Ui))
    );
    // The shell runs the command the click reported; so does `email.execute`.
    registry
        .execute("view.reader.markdown", &mut h.state_mut().0, Origin::Bus)
        .unwrap();
    h.run();
    assert_eq!(
        reader_info(&h.state().0),
        serde_json::json!({ "format": "plain", "auto": false })
    );
    assert!(h.query_by_label(MARKDOWN).is_some(), "the source, verbatim");
    assert!(h.query_by_label(&label("render-markdown")).is_some());
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
    let h = harness(opened("text/markdown", "Plain words *only*."), &theme(Scheme::Pro, Mode::Light));
    assert_eq!(reader_info(&h.state().0)["format"], "markdown");
    assert!(h.query_by_label("Plain words only.").is_some());
}

#[test]
fn a_plain_body_stays_plain() {
    let h = harness(opened("text/plain", PLAIN), &theme(Scheme::Pro, Mode::Light));
    assert!(h.query_by_label(PLAIN).is_some(), "one label, verbatim");
    assert!(h.query_by_label(&label("show-source")).is_none());
    assert!(h.query_by_label(&label("render-markdown")).is_none());
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
    assert!(registry.execute("view.reader.markdown", &mut { state }, Origin::Bus).is_err());
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

#[test]
fn reader_markdown_pro_light() {
    snapshot(
        "reader_markdown_pro_light",
        opened("text/plain", MARKDOWN),
        &theme(Scheme::Pro, Mode::Light),
    );
}

#[test]
fn reader_plain_pro_light() {
    snapshot(
        "reader_plain_pro_light",
        opened("text/plain", PLAIN),
        &theme(Scheme::Pro, Mode::Light),
    );
}
