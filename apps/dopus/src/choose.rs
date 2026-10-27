// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Chooser-only Bus host. Stage 2a deliberately has no production presenter.
//! The service still registers as `files` and answers well-formed choose
//! requests with UNAVAILABLE. It never registers the twin-pane commands.

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

fn defaults() -> Defaults {
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
    entries
}

fn describe() -> Value {
    json!({"contract":"ctk-app-control.v0", "app":SERVICE, "view":SERVICE,
        "title":crate::label("chooser-title"), "engine":"egui", "version":env!("CARGO_PKG_VERSION"),
        "description":"System file chooser provider", "controls":[],
        "commands":COMMANDS.iter().map(|(name, _)| *name).collect::<Vec<_>>()})
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
    answer: Answer,
    reply: impl std::future::Future<Output = bool>,
) -> Result<Option<Outcome>, String> {
    let sent = reply.await;
    if let Some(id) = answer.enable {
        if sent {
            engine.reply_sent(&id, presenter);
        } else {
            engine.reply_failed(&id, presenter);
            return Err("files could not send the choose reply; incarnation lost".into());
        }
    }
    Ok(answer.outcome)
}

async fn publish(client: &SupervisedClient, outcome: Outcome) {
    let headers = BTreeMap::from([("name".to_owned(), TOPIC.to_owned())]);
    let frame = citizen::outbox::frame(TOPIC, &json!(outcome));
    // A single attempt. Pulling the retained outcome recovers a missed event.
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
    let (_tx, events) = tokio::sync::mpsc::unbounded_channel();
    run_with_presenter(url, NoPresenter, events)
}

/// Stage 2b supplies its window presenter and event channel here. Only this
/// loop mutates the engine, replies to callers or publishes chosen outcomes.
pub fn run_with_presenter(
    url: &str,
    mut presenter: impl Presenter,
    mut events: tokio::sync::mpsc::UnboundedReceiver<Presentation>,
) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let client = tokio::time::timeout(Duration::from_secs(5),
            SupervisedClient::connect_options(SERVICE, url).fatal_on_registration_rejection(true).connect())
            .await.map_err(|_| "files connect timed out".to_owned())?.map_err(|e| e.to_string())?;
        let mut incoming = client.incoming().ok_or("files: no incoming channel")?;
        let mut state = client.subscribe_state();
        use std::os::unix::ffi::OsStrExt;
        let home = dopus_core::home_directory().as_os_str().as_bytes().to_vec();
        let mut engine = Engine::new(rand::random(), defaults(), home).map_err(|e| e.message)?;
        let start = Instant::now();
        let epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
        let mut events_open = true;
        loop {
            tokio::select! {
                cmd = incoming.recv() => {
                    let Some(cmd) = cmd else { return Err("files Bus closed".into()); };
                    if cmd.topic().is_some() || cmd.command.is_empty() { continue; }
                    citizen::debug::received(SERVICE, &cmd.command);
                    let now = epoch.saturating_add(start.elapsed().as_millis() as u64);
                    let answer = dispatch(&mut engine, &mut presenter, &cmd.command, &cmd.body, &citizen::caller_key(&cmd), now);
                    citizen::debug::answered(SERVICE, &cmd.command, answer.rc);
                    let (rc, body) = (answer.rc, answer.body.to_string());
                    let sent = async { matches!(tokio::time::timeout(Duration::from_secs(2), client.respond(&cmd, rc, &body)).await, Ok(Ok(()))) };
                    if let Some(outcome) = deliver(&mut engine, &mut presenter, answer, sent).await? {
                        publish(&client, outcome).await;
                    }
                }
                event = events.recv(), if events_open => {
                    if let Some(event) = event {
                        let now = epoch.saturating_add(start.elapsed().as_millis() as u64);
                        match engine.end(&event.request, event.end, now, &mut presenter) {
                            Ok(Some(outcome)) => publish(&client, outcome).await,
                            Ok(None) => {},
                            Err(error) => tracing::warn!("files presenter report refused: {}", error.message),
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
    fn no_presenter_refuses_and_keeps_nothing_and_no_dopus_commands_leak() {
        let mut engine = Engine::new(42, defaults(), b"/home/user".to_vec()).unwrap();
        let mut presenter = NoPresenter;
        for _ in 0..20 {
            let answer = dispatch(
                &mut engine,
                &mut presenter,
                "files.choose",
                "{}",
                "local:test",
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
            "files.ui.tree",
        ] {
            let answer = dispatch(&mut engine, &mut presenter, command, "{}", "test", 1);
            assert_eq!(answer.rc, 10);
            assert_eq!(answer.body["error_code"], "UNKNOWN_COMMAND");
        }
        let help = dispatch(&mut engine, &mut presenter, "HELP", "", "test", 1);
        assert_eq!(help.rc, 0);
        assert_eq!(help.body.as_array().unwrap().len(), 6);
        let described = dispatch(&mut engine, &mut presenter, "app.describe", "", "test", 1);
        assert_eq!(described.body["app"], "files");
        assert_eq!(described.body["commands"].as_array().unwrap().len(), 4);
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
                        end: End::Cancel,
                    })
                    .unwrap();
            }
            fn close(&mut self, _: &str) {
                self.log.lock().unwrap().push("close");
            }
        }
        let log = Arc::new(Mutex::new(Vec::new()));
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
        futures::executor::block_on(deliver(&mut engine, &mut presenter, answer, reply)).unwrap();
        let event = rx.try_recv().unwrap();
        assert!(
            engine
                .end(&event.request, event.end, 2, &mut presenter)
                .unwrap()
                .is_some()
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
            ["open-disabled", "reply", "input", "close", "publish"]
        );
        let answer = dispatch(&mut engine, &mut presenter, "files.choose", "{}", "caller", 4);
        let error = futures::executor::block_on(deliver(
            &mut engine,
            &mut presenter,
            answer,
            async { false },
        ))
        .unwrap_err();
        assert!(error.contains("incarnation lost"));
        assert!(rx.try_recv().is_err());
        assert_eq!(engine.list()["requests"], json!([]));
    }
}
