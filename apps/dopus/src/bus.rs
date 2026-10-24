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

use crate::verbs::Command;

/// Everything the Bus thread delivers to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
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
    Respond { id: u64, rc: u8, body: String },
    Subscribe(String),
    Look(settings::follow::Read),
    LookRetry { delay: Duration, generation: u64 },
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
                if citizen::macros::dispatch("dopus", &service, &cmd, client.clone(), &mut macro_jobs) { continue; }
                next_command += 1;
                let delivery = Delivery::Command(Command {
                    id: next_command,
                    verb: cmd.command.clone(),
                    body: if cmd.body.trim().is_empty() { "{}".to_owned() } else { cmd.body.clone() },
                });
                if cmd.id.is_some() {
                    commands.insert(next_command, cmd);
                }
                out.send(delivery);
            }
            next = work.recv() => {
                match next {
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
                            let call = c.call_with_headers_raw(read.service, read.verb, &headers, &body);
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
    verb: &str,
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
                .call_with_headers_raw(service, verb, &BTreeMap::new(), &body.to_string())
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
