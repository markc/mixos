// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus thread: a current-thread tokio runtime on its own OS thread,
//! holding a supervised client registered as `dopus` (or `--service NAME`)
//! that will not run on under a rejected registration. It forwards
//! `dopus.*` commands, topic frames and connection edges to the host, sends
//! the host's replies, subscribes topics, and performs the window's
//! session-look reads (`settings.get`, see `shell.rs`) and their retry
//! timers. A wake callback (the window's repaint request) runs after each
//! delivery, so nothing polls.
//!
//! No broker is not an error for the window: [`spawn`] returns
//! [`StartError::Unreachable`] and dopus runs without a Bus.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use ::bus::native_client::{ConnState, IncomingCommand, NodedClient, SupervisedClient};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use crate::commands::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailOutcome {
    pub added: bool,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailFailure {
    pub code: String,
    pub message: String,
}

impl MailFailure {
    pub fn unavailable() -> Self {
        Self {
            code: "UNAVAILABLE".into(),
            message: crate::label("mail-no-bus"),
        }
    }
}

impl From<citizen::AppCallError> for MailFailure {
    fn from(error: citizen::AppCallError) -> Self {
        let (code, message) = match error {
            citizen::AppCallError::Refused { code, message, .. } => (code, message),
            citizen::AppCallError::Transport(error) if error.outcome_unknown => (
                "OUTCOME_UNKNOWN".into(),
                crate::label("mail-outcome-unknown"),
            ),
            citizen::AppCallError::Transport(_) => {
                ("NOT_SENT".into(), crate::label("mail-not-sent"))
            }
            citizen::AppCallError::LaunchFailed(_) => {
                ("NOT_RUNNING".into(), crate::label("mail-launch-failed"))
            }
            citizen::AppCallError::DidNotAppear => {
                ("NOT_RUNNING".into(), crate::label("mail-did-not-appear"))
            }
        };
        Self { code, message }
    }
}

/// Runs on the Bus task. Only an explicit BUSY refusal switches to adding to
/// the current compose; a lost compose reply must never cause a second effect.
pub async fn mail_selection(
    client: &impl citizen::AppClient,
    paths: Vec<std::path::PathBuf>,
) -> Result<MailOutcome, MailFailure> {
    // Pane paths normally are absolute. Resolve a relative configured root
    // here, off the window thread, without canonicalising away symlink paths.
    let cwd = if paths.iter().any(|path| !path.is_absolute()) {
        std::env::current_dir().map_err(|error| MailFailure {
            code: "INVALID_ARGUMENT".into(),
            message: error.to_string(),
        })?
    } else {
        std::path::PathBuf::new()
    };
    let paths: Vec<_> = paths.into_iter().map(|path| cwd.join(path)).collect();
    let count = paths.len();
    match citizen::call_app(
        client,
        "dev.mixos.email",
        "email.compose",
        serde_json::json!({"attachments":paths}),
        Duration::from_secs(10),
    )
    .await
    {
        Ok(_) => Ok(MailOutcome {
            added: false,
            count,
        }),
        Err(citizen::AppCallError::Refused { code, .. }) if code == "BUSY" => {
            citizen::call_app(
                client,
                "dev.mixos.email",
                "email.compose.attachment.add",
                serde_json::json!({"paths":paths}),
                Duration::from_secs(10),
            )
            .await
            .map_err(MailFailure::from)?;
            Ok(MailOutcome { added: true, count })
        }
        Err(error) => Err(error.into()),
    }
}

/// The status line and Bus reply describe the same completed operation.
pub fn mail_reply(result: Result<MailOutcome, MailFailure>) -> (u8, String, String) {
    match result {
        Ok(outcome) => {
            let status = crate::label_with(
                if outcome.added {
                    "mail-added"
                } else {
                    "mail-opened"
                },
                &[("count", &outcome.count.to_string())],
            );
            let body = serde_json::json!({"id":"file.mail","ok":true,"result":{
                "added":outcome.added,"count":outcome.count,"message":status,
            }})
            .to_string();
            (0, body, status)
        }
        Err(error) => {
            let body =
                serde_json::json!({"error_code":error.code,"message":error.message}).to_string();
            (10, body, error.message)
        }
    }
}

/// Everything the Bus thread delivers to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    Mail {
        id: Option<u64>,
        result: Result<MailOutcome, MailFailure>,
    },
    /// A `dopus.*` command.
    Command(Command),
    /// A frame on a subscribed topic.
    Topic {
        topic: String,
        body: String,
    },
    Connected,
    Disconnected,
    /// The answer to session-look read `ticket`: `rc`, the body and the
    /// broker's error header, or `None` when the call failed or timed out.
    Look {
        ticket: u64,
        reply: Option<(u8, String, Option<String>)>,
    },
    /// A look retry armed with [`BusHandle::retry_look`] is due.
    LookRetry {
        generation: u64,
    },
    /// A subscription landed: frames on `topic` arrive from now on.
    Subscribed {
        topic: String,
    },
    /// The Bus thread finished.
    Stopped,
}

enum Work {
    Mail {
        id: Option<u64>,
        paths: Vec<std::path::PathBuf>,
    },
    Respond {
        id: u64,
        rc: u8,
        body: String,
    },
    Subscribe(String),
    Look(settings::follow::Read),
    LookRetry {
        delay: Duration,
        generation: u64,
    },
    Quit,
}

/// The host's handle on the Bus thread.
#[derive(Clone)]
pub struct BusHandle {
    tx: tokio::sync::mpsc::UnboundedSender<Work>,
    client: Arc<SupervisedClient>,
    /// Topic events waiting to be published.
    outbox: Arc<citizen::outbox::Outbox>,
    /// The Bus service name this handle registered.
    service: String,
}

impl BusHandle {
    pub fn mail(&self, id: Option<u64>, paths: Vec<std::path::PathBuf>) -> bool {
        self.tx.send(Work::Mail { id, paths }).is_ok()
    }
    pub fn connected(&self) -> bool {
        self.client.is_connected()
    }

    /// Queue `bodies` for publishing on `topic`, oldest first, without
    /// waiting; `lost` events were lost before them
    /// ([`citizen::outbox`]).
    pub fn publish(&self, topic: &str, bodies: Vec<serde_json::Value>, lost: u64) {
        self.outbox.push(topic, bodies, lost);
    }

    /// The Bus service name this handle registered.
    pub fn service(&self) -> &str {
        &self.service
    }

    /// Reply to command `id`.
    pub fn respond(&self, id: u64, rc: u8, body: String) {
        let _ = self.tx.send(Work::Respond { id, rc, body });
    }

    /// Subscribe `topic`; [`Delivery::Subscribed`] follows.
    pub fn subscribe(&self, topic: String) {
        let _ = self.tx.send(Work::Subscribe(topic));
    }

    /// Perform a session-look read; [`Delivery::Look`] answers it.
    pub fn read_look(&self, read: settings::follow::Read) {
        let _ = self.tx.send(Work::Look(read));
    }

    /// Deliver [`Delivery::LookRetry`] with `generation` after `delay`.
    pub fn retry_look(&self, delay: Duration, generation: u64) {
        let _ = self.tx.send(Work::LookRetry { delay, generation });
    }

    /// Send every queued reply, then stop. [`Delivery::Stopped`] follows.
    pub fn quit(&self) {
        let _ = self.tx.send(Work::Quit);
    }
}

/// Why the Bus could not be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// Another instance owns the service name.
    NameTaken,
    /// noded refused registration for another reason.
    Rejected(String),
    /// No broker reachable.
    Unreachable(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NameTaken => f.write_str("the service name is already registered"),
            StartError::Rejected(m) => write!(f, "registration refused: {m}"),
            StartError::Unreachable(m) => write!(f, "Bus unreachable: {m}"),
        }
    }
}

impl std::error::Error for StartError {}

/// Connect and register budget.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Single-instance probe deadline.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// One reply's budget.
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// How long one session-look read may take.
const LOOK_BUDGET: Duration = Duration::from_secs(30);

/// Wakes the host after each delivery (the window's repaint request).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
struct Out {
    tx: UnboundedSender<Delivery>,
    wake: Wake,
}

impl Out {
    fn send(&self, delivery: Delivery) {
        let _ = self.tx.unbounded_send(delivery);
        (self.wake)();
    }
}

/// Start the Bus thread registered as `service` on the broker at `url`.
pub fn spawn(
    service: &str,
    url: &str,
    wake: Wake,
) -> Result<(BusHandle, UnboundedReceiver<Delivery>), StartError> {
    let (dtx, drx) = unbounded();
    let (wtx, wrx) = tokio::sync::mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let outbox = Arc::new(citizen::outbox::Outbox::default());
    let worker_outbox = outbox.clone();
    let service = service.to_owned();
    let registered = service.clone();
    let url = url.to_owned();
    std::thread::Builder::new()
        .name(format!("{service}-bus"))
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ =
                        ready_tx.send(Err(StartError::Unreachable(format!("Bus runtime: {e}"))));
                    return;
                }
            };
            let out = Out { tx: dtx, wake };
            runtime.block_on(run(service, url, out, wrx, ready_tx, worker_outbox));
            runtime.shutdown_timeout(Duration::from_millis(100));
        })
        .map_err(|e| StartError::Unreachable(format!("Bus thread: {e}")))?;
    match ready_rx.recv() {
        Ok(Ok(client)) => Ok((
            BusHandle {
                tx: wtx,
                client,
                outbox,
                service: registered,
            },
            drx,
        )),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(StartError::Unreachable("the Bus thread exited".into())),
    }
}

async fn respond(client: &SupervisedClient, cmd: &IncomingCommand, rc: u8, body: &str) {
    citizen::debug::answered("dopus", &cmd.command, rc);
    if let Err(error) = tokio::time::timeout(REPLY_TIMEOUT, client.respond(cmd, rc, body))
        .await
        .map_err(|_| "timed out".to_owned())
        .and_then(|r| r.map_err(|e| e.to_string()))
    {
        tracing::warn!("dopus Bus reply: {error}");
    }
}

async fn run(
    service: String,
    url: String,
    out: Out,
    mut work: tokio::sync::mpsc::UnboundedReceiver<Work>,
    ready: std::sync::mpsc::Sender<Result<Arc<SupervisedClient>, StartError>>,
    outbox: Arc<citizen::outbox::Outbox>,
) {
    let connect = SupervisedClient::connect_options(&service, &url)
        .fatal_on_registration_rejection(true)
        .connect();
    let client = match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
        Ok(Ok(c)) => Arc::new(c),
        Ok(Err(e)) => {
            let err = match e.registration_rejection() {
                Some((_, msg)) if msg.contains("already registered") => StartError::NameTaken,
                Some((rc, msg)) => StartError::Rejected(format!("rc {rc}: {msg}")),
                None => StartError::Unreachable(e.to_string()),
            };
            let _ = ready.send(Err(err));
            return;
        }
        Err(_) => {
            let _ = ready.send(Err(StartError::Unreachable("connect timed out".into())));
            return;
        }
    };
    let Some(mut incoming) = client.incoming() else {
        let _ = ready.send(Err(StartError::Unreachable("no incoming channel".into())));
        return;
    };
    let mut state = client.subscribe_state();
    let _ = ready.send(Ok(Arc::clone(&client)));
    citizen::macros::install("dopus", &service, outbox.clone());
    citizen::outbox::spawn_publisher(client.clone(), outbox);

    // Commands awaiting a reply from the host.
    let mut commands: HashMap<u64, IncomingCommand> = HashMap::new();
    let mut next_command = 0u64;
    let mut macro_jobs = citizen::macros::Jobs::default();
    loop {
        tokio::select! {
            cmd = incoming.recv() => {
                let Some(cmd) = cmd else { break };
                if let Some(topic) = cmd.topic() {
                    out.send(Delivery::Topic { topic: topic.to_owned(), body: cmd.body.clone() });
                    continue;
                }
                if cmd.command.is_empty() {
                    continue;
                }
                citizen::debug::received(&service, &cmd.command);
                if let Some((rc, body)) = citizen::debug::request_for(&service, "dopus", &cmd.command, &cmd.body) {
                    respond(&client, &cmd, rc, &body.to_string()).await;
                    continue;
                }
                if citizen::macros::dispatch("dopus", &cmd, client.clone(), &mut macro_jobs) { continue; }
                next_command += 1;
                let delivery = Delivery::Command(Command {
                    id: next_command,
                    command: cmd.command.clone(),
                    body: if cmd.body.trim().is_empty() { "{}".to_owned() } else { cmd.body.clone() },
                });
                if cmd.id.is_some() {
                    commands.insert(next_command, cmd);
                }
                out.send(delivery);
            }
            next = work.recv() => {
                match next {
                    Some(Work::Mail { id, paths }) => {
                        let (c, o) = (client.clone(), out.clone());
                        tokio::spawn(async move {
                            let result = mail_selection(c.as_ref(), paths).await;
                            o.send(Delivery::Mail { id, result });
                        });
                    }
                    None => break,
                    Some(Work::Respond { id, rc, body }) => {
                        // Awaited in place, so a reply is on the wire before a
                        // later quit closes the client.
                        if let Some(cmd) = commands.remove(&id) {
                            respond(&client, &cmd, rc, &body).await;
                        }
                    }
                    Some(Work::Subscribe(topic)) => {
                        let (c, o) = (client.clone(), out.clone());
                        tokio::spawn(async move {
                            // The client replays only topics that once
                            // subscribed: retry until this one lands.
                            let mut delay = Duration::from_millis(250);
                            loop {
                                if matches!(c.state(), ConnState::ShuttingDown | ConnState::Fatal) {
                                    return;
                                }
                                match c.subscribe_topic(&topic).await {
                                    Ok(_) => break,
                                    Err(e) => {
                                        tracing::warn!("subscribe {topic}: {e}; retrying");
                                        tokio::time::sleep(delay).await;
                                        delay = (delay * 2).min(Duration::from_secs(5));
                                    }
                                }
                            }
                            o.send(Delivery::Subscribed { topic });
                        });
                    }
                    Some(Work::Look(read)) => {
                        let (c, o) = (client.clone(), out.clone());
                        tokio::spawn(async move {
                            let (body, headers) = (read.body.to_string(), BTreeMap::new());
                            let call = c.call_with_headers_raw(read.service, read.command, &headers, &body);
                            let reply = match tokio::time::timeout(LOOK_BUDGET, call).await {
                                Ok(Ok(answer)) => Some(answer),
                                _ => None,
                            };
                            o.send(Delivery::Look { ticket: read.ticket, reply });
                        });
                    }
                    Some(Work::LookRetry { delay, generation }) => {
                        let o = out.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(delay).await;
                            o.send(Delivery::LookRetry { generation });
                        });
                    }
                    Some(Work::Quit) => {
                        // Replies queued behind the quit still go out.
                        while let Ok(Work::Respond { id, rc, body }) = work.try_recv() {
                            if let Some(cmd) = commands.remove(&id) {
                                respond(&client, &cmd, rc, &body).await;
                            }
                        }
                        break;
                    }
                }
            }
            changed = state.changed() => {
                if changed.is_err() {
                    break;
                }
                let edge = match *state.borrow_and_update() {
                    ConnState::Connected => Some(Delivery::Connected),
                    ConnState::Disconnected => Some(Delivery::Disconnected),
                    ConnState::ShuttingDown | ConnState::Fatal => break,
                    ConnState::Connecting => None,
                };
                if let Some(edge) = edge {
                    out.send(edge);
                }
            }
        }
    }
    // No caller is left waiting on a closed dopus.
    let closing = r#"{"error_code":"BUSY","message":"dopus is closing"}"#;
    for (_, cmd) in commands.drain() {
        respond(&client, &cmd, 10, closing).await;
    }
    macro_jobs.finish().await;
    let _ = tokio::time::timeout(REPLY_TIMEOUT, client.close()).await;
    out.send(Delivery::Stopped);
}

/// One anonymous request to `service`, bounded by `limit`.
fn anonymous_call(
    url: &str,
    service: &str,
    command_name: &str,
    body: &serde_json::Value,
    limit: Duration,
) -> Option<(u8, String)> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime.block_on(async {
        let call = async {
            let client = NodedClient::connect_anonymous(url).await.ok()?;
            let reply = client
                .call_with_headers_raw(service, command_name, &BTreeMap::new(), &body.to_string())
                .await
                .ok();
            client.close().await;
            reply.map(|(rc, body, _)| (rc, body))
        };
        tokio::time::timeout(limit, call).await.ok().flatten()
    })
}

/// Whether an instance answers `dopus.ping` on `service` within 500 ms.
pub fn probe_running(url: &str, service: &str) -> bool {
    matches!(
        anonymous_call(
            url,
            service,
            "dopus.ping",
            &serde_json::json!({}),
            PROBE_TIMEOUT
        ),
        Some((0, _))
    )
}

/// Hand the paths to the running instance (`dopus.open`).
pub fn forward_open(url: &str, service: &str, paths: &[String]) -> Result<(), String> {
    match anonymous_call(
        url,
        service,
        "dopus.open",
        &serde_json::json!({ "paths": paths }),
        Duration::from_secs(5),
    ) {
        Some((0, _)) => Ok(()),
        Some((rc, body)) => Err(format!("dopus.open refused (rc {rc}): {body}")),
        None => Err(format!("no answer from {service}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citizen::{Handle, Reply};
    use serde_json::{Value, json};

    fn reply(rc: u8, body: Value) -> Result<Reply, citizen::CallError> {
        Ok(Reply {
            rc,
            body: body.to_string(),
            error: None,
        })
    }

    fn mail(client: &Handle) -> Result<MailOutcome, MailFailure> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(mail_selection(
                client,
                vec!["/home/user/one".into(), "/home/user/two".into()],
            ))
    }

    #[test]
    fn app_call_failures_use_the_catalogue_and_preserve_uncertainty() {
        use citizen::{AppCallError, CallError};

        for (error, code, key) in [
            (
                AppCallError::LaunchFailed("internal detail".into()),
                "NOT_RUNNING",
                "mail-launch-failed",
            ),
            (
                AppCallError::DidNotAppear,
                "NOT_RUNNING",
                "mail-did-not-appear",
            ),
            (
                AppCallError::Transport(CallError {
                    message: "internal detail".into(),
                    outcome_unknown: false,
                }),
                "NOT_SENT",
                "mail-not-sent",
            ),
            (
                AppCallError::Transport(CallError {
                    message: "app call timed out; outcome unknown".into(),
                    outcome_unknown: true,
                }),
                "OUTCOME_UNKNOWN",
                "mail-outcome-unknown",
            ),
        ] {
            let (rc, body, status) = mail_reply(Err(error.into()));
            assert_eq!(rc, 10);
            assert_eq!(
                serde_json::from_str::<Value>(&body).unwrap()["error_code"],
                code
            );
            assert_eq!(status, crate::label(key));
            assert_ne!(status, key, "the Fluent key must exist");
            assert!(!status.contains("internal detail"));
        }
    }

    #[test]
    fn busy_falls_back_to_add_with_the_same_paths() {
        let client = Handle::scripted(vec![
            reply(10, json!({"error_code":"BUSY","message":"compose open"})),
            reply(0, json!({"compose":{}})),
        ]);
        let outcome = mail(&client).unwrap();
        assert_eq!(
            outcome,
            MailOutcome {
                added: true,
                count: 2
            }
        );
        let calls: Vec<_> = client
            .log()
            .into_iter()
            .filter_map(|entry| match entry {
                citizen::Logged::Call { command, body, .. } => {
                    Some((command, serde_json::from_str::<Value>(&body).unwrap()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(calls[0].0, "email.compose");
        assert_eq!(calls[1].0, "email.compose.attachment.add");
        assert_eq!(calls[0].1["attachments"], calls[1].1["paths"]);
        let (rc, body, status) = mail_reply(Ok(outcome));
        assert_eq!(rc, 0);
        assert_eq!(status, "Added 2 files to the message being written");
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["result"]["message"],
            status
        );
    }

    #[test]
    fn opened_status_and_refusals_are_returned_without_other_calls() {
        let client = Handle::scripted(vec![reply(0, json!({}))]);
        let (rc, _, status) = mail_reply(mail(&client));
        assert_eq!(rc, 0);
        assert_eq!(status, "Opened a new message with 2 files");
        assert_eq!(client.log().len(), 1);
        let client = Handle::scripted(vec![reply(
            10,
            json!({"error_code":"ATTACHMENT_UNREADABLE","message":"cannot read file"}),
        )]);
        let (rc, body, status) = mail_reply(mail(&client));
        assert_eq!(rc, 10);
        assert_eq!(status, "cannot read file");
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["error_code"],
            "ATTACHMENT_UNREADABLE"
        );
        assert_eq!(client.log().len(), 1);
    }
}
