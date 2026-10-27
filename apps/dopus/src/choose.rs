// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Chooser-only Bus host and secondary native viewports. No twin-pane commands.

mod placement;
mod presenter;
pub mod window;

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ::bus::native_client::{ConnState, SupervisedClient};
use dopus_core::chooser::{
    COMMANDS, Defaults, End, Engine, Outcome, Presenter, Refusal, Unavailable,
};
use serde_json::{Value, json};

pub const SERVICE: &str = "files";
pub const TOPIC: &str = "files.chosen";

struct NoPresenter;

impl Presenter for NoPresenter {
    fn open(&mut self, _: &dopus_core::chooser::OpenRequest) -> Result<(), Unavailable> {
        Err(Unavailable(crate::label("chooser-unavailable")))
    }
    fn input_ready(&mut self, _: &str) {}
    fn close(&mut self, _: &str) {}
}

pub fn defaults() -> Defaults {
    Defaults {
        open_title: crate::label("chooser-open-title"),
        save_title: crate::label("chooser-save-title"),
        save_files_title: crate::label("chooser-save-files-title"),
        open_accept: crate::label("chooser-open-accept"),
        save_accept: crate::label("chooser-save-accept"),
    }
}

pub fn manifest() -> Vec<::bus::CommandDescriptor> {
    let mut entries = vec![
        ::bus::CommandDescriptor::new("HELP", &[], "List every command", true),
        ::bus::CommandDescriptor::new("app.describe", &[], "Describe the file chooser", true),
    ];
    for (name, read_only) in COMMANDS {
        let args: &[&str] = match *name {
            "files.choose" => &[
                "mode?",
                "title?",
                "accept_label?",
                "multiple?",
                "directory?",
                "filters?",
                "current_filter?",
                "choices?",
                "current_folder?",
                "current_name?",
                "current_file?",
                "files?",
                "parent_window?",
                "correlation?",
                "on_behalf_of?",
            ],
            "files.choose.result" | "files.choose.cancel" => &["request"],
            _ => &[],
        };
        entries.push(::bus::CommandDescriptor::new(name, args, name, *read_only));
    }
    for entry in toolkit::drive::describe(SERVICE) {
        entries.push(::bus::CommandDescriptor::new(
            entry["name"].as_str().unwrap(),
            &["viewport?"],
            entry["description"].as_str().unwrap_or_default(),
            entry["read_only"].as_bool().unwrap_or(false),
        ));
    }
    entries
}

fn describe() -> Value {
    json!({"contract":"ctk-app-control.v0", "app":SERVICE, "view":SERVICE,
        "title":crate::label("chooser-title"), "engine":"egui", "version":env!("CARGO_PKG_VERSION"),
        "description":"System file chooser provider", "controls":[],
        "commands":manifest().into_iter().filter(|e| e.name.starts_with("files.")).map(|e| e.name).collect::<Vec<_>>()})
}

/// A prepared answer and, for choose only, the request whose input can be
/// enabled after the transport successfully sends the answer.
pub struct Answer {
    pub rc: u8,
    pub body: Value,
    pub enable: Option<String>,
    pub outcome: Option<dopus_core::chooser::Outcome>,
}

/// A presenter's asynchronous report, delivered on the provider's event loop.
pub struct Presentation {
    pub request: String,
    pub end: End,
}

/// The actual host ordering, shared with the immediate-answer test presenter.
async fn deliver(
    engine: &mut Engine,
    presenter: &mut impl Presenter,
    placement: &mut placement::Placement,
    mapping: &mut Mapping,
    answer: Answer,
    reply: impl std::future::Future<Output = bool>,
) -> Option<Outcome> {
    let sent = reply.await;
    if let Some(id) = answer.enable {
        if sent {
            engine.reply_sent(&id, presenter);
        } else {
            placement.closed(&id);
            mapping.deadlines.remove(&id);
            engine.reply_failed(&id, presenter);
            tracing::warn!(
                request = id,
                "files could not send the choose reply; request lost"
            );
        }
    }
    answer.outcome
}

fn end_presentation(
    engine: &mut Engine,
    presenter: &mut impl Presenter,
    event: Presentation,
    now: u64,
) -> Option<Outcome> {
    match engine.end(&event.request, event.end, now, presenter) {
        Ok(outcome) => outcome,
        Err(error) => {
            presenter.refused(&event.request, &error);
            tracing::warn!("files presenter report refused: {}", error.message);
            None
        }
    }
}

#[derive(Default)]
struct Mapping {
    deadlines: BTreeMap<String, tokio::time::Instant>,
}

impl Mapping {
    fn opened(&mut self, id: &str) {
        self.deadlines.insert(
            id.into(),
            tokio::time::Instant::now() + Duration::from_secs(5),
        );
    }

    async fn expired(&mut self) -> Vec<String> {
        let Some(deadline) = self.deadlines.values().min().copied() else {
            return std::future::pending().await;
        };
        tokio::time::sleep_until(deadline).await;
        let now = tokio::time::Instant::now();
        let ids: Vec<_> = self
            .deadlines
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.deadlines.remove(id);
        }
        ids
    }
}

fn fail_unmapped(
    engine: &mut Engine,
    presenter: &mut impl Presenter,
    id: &str,
    now: u64,
) -> Option<Outcome> {
    if presenter.mapped(id) {
        return None;
    }
    end_presentation(
        engine,
        presenter,
        Presentation {
            request: id.into(),
            end: End::Failed(format!(
                "UNAVAILABLE: {}",
                crate::label("chooser-unavailable")
            )),
        },
        now,
    )
}

fn publication_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("name".to_owned(), TOPIC.to_owned()),
        ("retain".to_owned(), "false".to_owned()),
    ])
}

async fn publish(client: &SupervisedClient, outcome: Outcome) {
    let headers = publication_headers();
    let frame = citizen::outbox::frame(TOPIC, &json!(outcome));
    // A single attempt. The engine's ten-minute result store recovers a missed event.
    if !matches!(
        tokio::time::timeout(
            Duration::from_secs(2),
            client.call_with_headers_raw("noded", "topic.publish", &headers, &frame)
        )
        .await,
        Ok(Ok((0, _, _)))
    ) {
        tracing::warn!("files outcome publication failed");
    }
}

/// This dispatch is also used by tests; transport ordering stays in the host.
pub fn dispatch(
    engine: &mut Engine,
    presenter: &mut impl Presenter,
    command: &str,
    body: &str,
    caller: &str,
    now: u64,
) -> Answer {
    let mut outcome = None;
    let mut enable = None;
    let result = match command {
        "HELP" => Ok(serde_json::to_value(manifest()).expect("manifest serialises")),
        "app.describe" => Ok(describe()),
        "files.choose" => engine
            .choose(body, caller, now, presenter)
            .inspect(|reply| {
                enable = reply["request"].as_str().map(str::to_owned);
            }),
        "files.choose.result" => engine.result(body, now),
        "files.choose.cancel" => engine.cancel(body, now, presenter).map(|ended| {
            outcome = ended;
            json!({})
        }),
        "files.choose.list" => {
            engine.sweep(now);
            Ok(engine.list())
        }
        _ => Err(Refusal::new(
            "UNKNOWN_COMMAND",
            "unknown file chooser command",
        )),
    };
    match result {
        Ok(body) => Answer {
            rc: 0,
            body,
            enable,
            outcome,
        },
        Err(refusal) => Answer {
            rc: 10,
            body: json!(refusal),
            enable: None,
            outcome: None,
        },
    }
}

/// No window, no desktop or file-manager workers. Fatal Bus loss exits so the
/// session unit restarts with a new incarnation and no surviving requests.
pub fn run(url: &str) -> Result<(), String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("DISPLAY").is_some_and(|s| !s.is_empty())
    {
        return presenter::run(url);
    }
    let (_tx, events) = tokio::sync::mpsc::unbounded_channel();
    run_with_presenter(url, NoPresenter, events)
}

/// Stage 2b supplies its window presenter and event channel here. Only this
/// loop mutates the engine, replies to callers or publishes chosen outcomes.
pub fn run_with_presenter(
    url: &str,
    presenter: impl Presenter,
    events: tokio::sync::mpsc::UnboundedReceiver<Presentation>,
) -> Result<(), String> {
    run_host(url, presenter, events, None)
}

fn run_host(
    url: &str,
    mut presenter: impl Presenter,
    mut events: tokio::sync::mpsc::UnboundedReceiver<Presentation>,
    mut driver: Option<presenter::DriveHost>,
) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let client = std::sync::Arc::new(tokio::time::timeout(Duration::from_secs(5),
            SupervisedClient::connect_options(SERVICE, url).fatal_on_registration_rejection(true).connect())
            .await.map_err(|_| "files connect timed out".to_owned())?.map_err(|e| e.to_string())?);
        let mut incoming = client.incoming().ok_or("files: no incoming channel")?;
        let mut state = client.subscribe_state();
        use std::os::unix::ffi::OsStrExt;
        let home = dopus_core::home_directory().as_os_str().as_bytes().to_vec();
        let mut engine = Engine::new(rand::random(), defaults(), home).map_err(|e| e.message)?;
        let start = Instant::now();
        let epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
        let mut events_open = true;
        let mut placement = placement::Placement::new();
        let mut mapping = Mapping::default();
        if driver.is_some() { let _ = client.subscribe_topic(&placement.topic()).await; }
        let mut drive_id = 0u64;
        let mut pending = BTreeMap::new();
        loop {
            tokio::select! {
                cmd = incoming.recv() => {
                    let Some(cmd) = cmd else { return Err("files Bus closed".into()); };
                    if cmd.topic() == Some(placement.topic().as_str()) { placement.changed(client.clone()); continue; }
                    if cmd.topic().is_some() || cmd.command.is_empty() { continue; }
                    citizen::debug::received(SERVICE, &cmd.command);
                    if let Some(command) = cmd.command.strip_prefix("files.").filter(|c| toolkit::drive::COMMANDS.contains(c)) {
                        if let Some(driver) = &driver {
                            drive_id += 1;
                            let args = driver.target(serde_json::from_str(&cmd.body).unwrap_or(Value::Null));
                            let args = match args {
                                Ok(args) => args,
                                Err(message) => {
                                    let _ = client.respond(&cmd, 10, &json!({"error_code":"ARGUMENT", "message":message}).to_string()).await;
                                    continue;
                                }
                            };
                            let request = presenter::DriveRequest { id: drive_id, command: command.into(), args };
                            if driver.requests.send(request).is_ok() {
                                pending.insert(drive_id, cmd);
                                driver.ctx.request_repaint_of(egui::ViewportId::ROOT);
                                continue;
                            }
                        }
                        let _ = client.respond(&cmd, 10, &json!({"error_code":"UNAVAILABLE","message":crate::label("chooser-unavailable")}).to_string()).await;
                        continue;
                    }
                    let now = epoch.saturating_add(start.elapsed().as_millis() as u64);
                    let answer = dispatch(&mut engine, &mut presenter, &cmd.command, &cmd.body, &citizen::caller_key(&cmd), now);
                    if let Some(id) = &answer.enable {
                        mapping.opened(id);
                        if driver.is_some() { placement.opened(id.clone(), client.clone()); }
                    }
                    if let Some(outcome) = &answer.outcome { placement.closed(&outcome.request); mapping.deadlines.remove(&outcome.request); }
                    citizen::debug::answered(SERVICE, &cmd.command, answer.rc);
                    let (rc, body) = (answer.rc, answer.body.to_string());
                    let sent = async { matches!(tokio::time::timeout(Duration::from_secs(2), client.respond(&cmd, rc, &body)).await, Ok(Ok(()))) };
                    if let Some(outcome) = deliver(&mut engine, &mut presenter, &mut placement, &mut mapping, answer, sent).await {
                        publish(&client, outcome).await;
                    }
                }
                id = placement.completed(&client) => {
                    if let Some(id) = id && let Some(driver) = &driver { driver.allow_map(&id); }
                }
                ids = mapping.expired() => {
                    let now = epoch.saturating_add(start.elapsed().as_millis() as u64);
                    for id in ids {
                        if let Some(outcome) = fail_unmapped(&mut engine, &mut presenter, &id, now) {
                            placement.closed(&id);
                            publish(&client, outcome).await;
                        }
                    }
                }
                answer = async { match &mut driver { Some(driver) => driver.answers.recv().await, None => std::future::pending().await } } => {
                    match answer {
                        Some(presenter::Report::Drive(answer)) => if let Some(cmd) = pending.remove(&answer.id) {
                            let _ = client.respond(&cmd, answer.rc, &answer.body.to_string()).await;
                        },
                        Some(presenter::Report::Executed(bodies)) => {
                            let topic = toolkit::executed::topic(SERVICE);
                            for body in bodies {
                                let headers = BTreeMap::from([("name".into(), topic.clone()), ("retain".into(), "false".into())]);
                                let frame = citizen::outbox::frame(&topic, &body);
                                let _ = client.call_with_headers_raw("noded", "topic.publish", &headers, &frame).await;
                            }
                        }
                        None => {}
                    }
                }
                event = events.recv(), if events_open => {
                    if let Some(event) = event {
                        let now = epoch.saturating_add(start.elapsed().as_millis() as u64);
                        if let Some(outcome) = end_presentation(&mut engine, &mut presenter, event, now) {
                            placement.closed(&outcome.request);
                            mapping.deadlines.remove(&outcome.request);
                            publish(&client, outcome).await;
                        }
                    } else { events_open = false; }
                }
                changed = state.changed() => {
                    if changed.is_err() || *state.borrow_and_update() != ConnState::Connected {
                        return Err("files Bus disconnected".into());
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_publication_explicitly_disables_broker_retention() {
        assert_eq!(
            publication_headers(),
            BTreeMap::from([
                ("name".into(), "files.chosen".into()),
                ("retain".into(), "false".into()),
            ])
        );
    }

    #[test]
    fn no_presenter_refuses_and_keeps_nothing_and_no_dopus_commands_leak() {
        let mut engine = Engine::new(42, defaults(), b"/home/user".to_vec()).unwrap();
        let mut presenter = NoPresenter;
        for i in 0..20 {
            let answer = dispatch(
                &mut engine,
                &mut presenter,
                "files.choose",
                "{}",
                &format!("local:test-{i}"),
                1,
            );
            assert_eq!(answer.rc, 10);
            assert_eq!(answer.body["error_code"], "UNAVAILABLE");
            assert!(answer.enable.is_none());
        }
        assert_eq!(engine.list()["requests"], json!([]));
        let invalid = dispatch(
            &mut engine,
            &mut presenter,
            "files.choose",
            r#"{"mode":"bad"}"#,
            "test",
            1,
        );
        assert_eq!(invalid.rc, 10);
        assert_eq!(invalid.body["error_code"], "ARGUMENT");
        for command in [
            "dopus.ping",
            "dopus.quit",
            "dopus.open",
            "dopus.macros.list",
        ] {
            let answer = dispatch(&mut engine, &mut presenter, command, "{}", "test", 1);
            assert_eq!(answer.rc, 10);
            assert_eq!(answer.body["error_code"], "UNKNOWN_COMMAND");
        }
        let help = dispatch(&mut engine, &mut presenter, "HELP", "", "test", 1);
        assert_eq!(help.rc, 0);
        assert_eq!(help.body.as_array().unwrap().len(), 22);
        let described = dispatch(&mut engine, &mut presenter, "app.describe", "", "test", 1);
        assert_eq!(described.body["app"], "files");
        assert_eq!(described.body["commands"].as_array().unwrap().len(), 20);
    }

    #[test]
    fn immediate_presenter_cannot_publish_before_the_reply_and_only_publishes_once() {
        use std::sync::{Arc, Mutex};
        struct Immediate {
            log: Arc<Mutex<Vec<&'static str>>>,
            events: tokio::sync::mpsc::UnboundedSender<Presentation>,
        }
        impl Presenter for Immediate {
            fn open(&mut self, _: &dopus_core::chooser::OpenRequest) -> Result<(), Unavailable> {
                self.log.lock().unwrap().push("open-disabled");
                Ok(())
            }
            fn input_ready(&mut self, id: &str) {
                self.log.lock().unwrap().push("input");
                self.events
                    .send(Presentation {
                        request: id.into(),
                        end: End::Accept(dopus_core::chooser::Acceptance {
                            paths: vec![b"/tmp/first".to_vec()],
                            folder: None,
                            current_filter: None,
                            choices: BTreeMap::new(),
                        }),
                    })
                    .unwrap();
            }
            fn close(&mut self, _: &str) {
                self.log.lock().unwrap().push("close");
            }
        }
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut placement = placement::Placement::new();
        let mut mapping = Mapping::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut presenter = Immediate {
            log: log.clone(),
            events: tx,
        };
        let mut engine = Engine::new(1, defaults(), b"/home/user".to_vec()).unwrap();
        let answer = dispatch(
            &mut engine,
            &mut presenter,
            "files.choose",
            "{}",
            "caller",
            1,
        );
        assert_eq!(answer.rc, 0);
        assert!(rx.try_recv().is_err());
        let reply = async {
            log.lock().unwrap().push("reply");
            true
        };
        assert!(
            futures::executor::block_on(deliver(
                &mut engine,
                &mut presenter,
                &mut placement,
                &mut mapping,
                answer,
                reply
            ))
            .is_none()
        );
        // A second request's failed (or timed-out) reply cannot lose the first.
        let answer = dispatch(
            &mut engine,
            &mut presenter,
            "files.choose",
            "{}",
            "caller",
            1,
        );
        let lost = answer.body["request"].as_str().unwrap().to_owned();
        assert!(
            futures::executor::block_on(deliver(
                &mut engine,
                &mut presenter,
                &mut placement,
                &mut mapping,
                answer,
                async { false }
            ))
            .is_none()
        );
        assert_eq!(rx.len(), 1);
        let lost_result = dispatch(
            &mut engine,
            &mut presenter,
            "files.choose.result",
            &json!({"request":lost}).to_string(),
            "caller",
            2,
        );
        assert_eq!(lost_result.rc, 10);
        assert_eq!(lost_result.body["error_code"], "ARGUMENT");
        assert_eq!(lost_result.body["reason"], "unknown");
        assert_eq!(engine.list()["requests"].as_array().unwrap().len(), 1);
        let event = rx.try_recv().unwrap();
        let outcome = engine
            .end(&event.request, event.end, 2, &mut presenter)
            .unwrap()
            .unwrap();
        assert_eq!(outcome.response, 0);
        assert_eq!(outcome.uris.as_ref().unwrap(), &["file:///tmp/first"]);
        assert_eq!(
            engine
                .result(&json!({"request":event.request}).to_string(), 3)
                .unwrap()["outcome"],
            json!(outcome)
        );
        log.lock().unwrap().push("publish");
        assert!(
            engine
                .end(&event.request, End::Caller, 3, &mut presenter)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            *log.lock().unwrap(),
            [
                "open-disabled",
                "reply",
                "input",
                "open-disabled",
                "close",
                "close",
                "publish"
            ]
        );
        let answer = dispatch(
            &mut engine,
            &mut presenter,
            "files.choose",
            "{}",
            "caller",
            4,
        );
        assert!(
            futures::executor::block_on(deliver(
                &mut engine,
                &mut presenter,
                &mut placement,
                &mut mapping,
                answer,
                async { false }
            ))
            .is_none()
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(engine.list()["requests"], json!([]));
    }
}
