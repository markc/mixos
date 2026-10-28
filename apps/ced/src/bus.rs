// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Bus thread: a current-thread tokio runtime on its own OS thread,
//! holding a supervised client registered as `ced` (or `--service NAME`)
//! that refuses to run on under a rejected registration. It correlates
//! requests, arms one-shot timers and deadlines, subscribes topics
//! (`edit.changed`, `theme.changed`, `noded.props.changed`) with their
//! bodies, forwards connection edges, and hands `ced.*` commands to the
//! controller. It also performs the window's own session-look reads
//! (`settings.get`, see `shell.rs`) and their retry timers, which never
//! reach the controller. Deliveries reach the host through an unbounded
//! channel, and a wake callback (the window's repaint request) runs after
//! each one, so nothing polls.
//!
//! Every wait here is an event: a Bus frame, an effect from the host, a
//! connection-state edge, or a timer the controller armed. Requests go out
//! with `call_with_headers_raw`, so a refusal's whole `{error_code, message,
//! reason, …}` body reaches the mirror. A request that fails on the
//! transport (disconnected) is reported as its `Deadline` only once its
//! deadline has passed, so a mirror reconciling while the broker is down
//! retries at most once per deadline, never in a hot loop.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use ::bus::native_client::{
    BoundedIncomingEvent, ConnState, IncomingCommand, NodedClient, SupervisedClient,
    SupervisedError,
};
use documents::controller::{BusCommand, Effect};
use documents::session::SessionWriter;
use editor_model::types::{Incoming, ParsedBody};
use futures::FutureExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

/// The commands this host answers: engine, shared macros, then window drive.
pub fn help(window: bool) -> Vec<::bus::CommandDescriptor> {
    let mut entries = documents::commands::manifest();
    let mut hosted = citizen::macros::describe("ced");
    if window {
        hosted.extend(toolkit::drive::describe("ced"));
    }
    entries.extend(hosted.into_iter().map(|value| {
        ::bus::CommandDescriptor {
            name: value["name"].as_str().unwrap().to_owned(),
            args: value["args"]
                .as_object()
                .map(|args| args.keys().cloned().collect())
                .unwrap_or_default(),
            description: value["description"].as_str().unwrap().to_owned(),
            read_only: value["read_only"].as_bool().unwrap_or(false),
        }
    }));
    entries.extend(citizen::debug::describe("ced").into_iter().map(|value| {
        ::bus::CommandDescriptor::new(
            value["name"].as_str().unwrap(),
            if value["read_only"] == true {
                &[]
            } else {
                &["level?", "on?"]
            },
            value["description"].as_str().unwrap(),
            value["read_only"].as_bool().unwrap(),
        )
    }));
    entries
}

/// Everything the Bus thread delivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    MailSaved { tab: u64, path: String, text: String, send: bool, command: Option<(u64, String)>, result: Result<(), String> },
    MailSent { tab: u64, command: Option<(u64, String)>, result: Result<String, String> },
    NotificationFailed {
        text: String,
    },
    Incoming(Incoming),
    Command(BusCommand),
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
    /// A `Subscribe` effect landed: deliveries on `topic` now arrive, so a
    /// read made from here on cannot miss a change.
    Subscribed {
        topic: String,
    },
    /// The Bus thread finished; anything it could not do cleanly.
    Stopped {
        faults: Vec<String>,
    },
}

enum WorkerCommand {
    Effect(Effect),
    Look(settings::follow::Read),
    LookRetry { delay: Duration, generation: u64 },
    Shutdown(Option<SessionWriter>),
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

/// The service every mirror request goes to.
const EDIT: &str = "edit";
/// Success replies bigger than this are parsed on the Bus thread, not the
/// UI thread (snapshot pages are up to 4 MiB).
const PARSE_OFF_UI_BYTES: usize = 64 * 1024;
/// Connect and register budget.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Single-instance probe deadline.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// Shutdown drains replies and the session within this budget.
const DRAIN: Duration = Duration::from_secs(2);
/// How long one session-look read may take.
const LOOK_BUDGET: Duration = Duration::from_secs(30);

/// Broker errors can have only an error header and an empty body/message.
/// Preserve a real sentence at the boundary, including the command and code.
fn failure_body(rc: u8, body: String, error: Option<&str>, command_name: &str) -> String {
    if rc < 10 {
        return body;
    }
    let mut value = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({"message": body}));
    let code = value["error_code"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(
            if error.is_some_and(|s| s.starts_with("Service '") && s.ends_with("' not found")) {
                "UNAVAILABLE"
            } else {
                "INTERNAL"
            },
        )
        .to_owned();
    if value["message"]
        .as_str()
        .is_none_or(|s| s.trim().is_empty())
    {
        value["message"] = serde_json::json!(match error.filter(|s| !s.trim().is_empty()) {
            Some(error) => format!("{command_name} failed ({code}): {error}."),
            None => format!("{command_name} failed ({code})."),
        });
    }
    value["error_code"] = serde_json::json!(code);
    value.to_string()
}

/// Wakes the host after each delivery (the window's repaint request).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// The host's handle for the [`Effect`]s that need the Bus (sends, replies,
/// timers, subscriptions, quit). Other effects are the host's own.
pub struct BusHandle {
    tx: tokio::sync::mpsc::UnboundedSender<WorkerCommand>,
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

    /// Stop: answer what is queued, flush `session`, close the connection.
    /// [`Delivery::Stopped`] follows.
    pub fn shutdown(&self, session: Option<SessionWriter>) {
        let _ = self.tx.send(WorkerCommand::Shutdown(session));
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

    /// Hand a Bus effect to the Bus thread; any other effect is ignored.
    pub fn perform(&self, effect: &Effect) {
        if is_bus(effect) {
            let _ = self.tx.send(WorkerCommand::Effect(effect.clone()));
        }
    }

    /// Perform a session-look read; [`Delivery::Look`] answers it.
    pub fn read_look(&self, read: settings::follow::Read) {
        let _ = self.tx.send(WorkerCommand::Look(read));
    }

    /// Deliver [`Delivery::LookRetry`] with `generation` after `delay`.
    pub fn retry_look(&self, delay: Duration, generation: u64) {
        let _ = self.tx.send(WorkerCommand::LookRetry { delay, generation });
    }
}

/// Whether the Bus thread performs `effect`.
pub fn is_bus(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::SaveMail { .. } | Effect::SendMail { .. } | Effect::Notify { .. }
            | Effect::Send { .. }
            | Effect::Respond { .. }
            | Effect::Timer { .. }
            | Effect::Subscribe { .. }
            | Effect::Quit
    )
}

pub use citizen::caller_key;

/// Start the Bus thread registered as `service` on the broker at `url`.
pub fn spawn(
    service: &str,
    url: &str,
    wake: Wake,
) -> Result<(BusHandle, UnboundedReceiver<Delivery>), StartError> {
    let (dtx, drx) = unbounded();
    let (etx, erx) = tokio::sync::mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let outbox = Arc::new(citizen::outbox::Outbox::default());
    let worker_outbox = outbox.clone();
    let service = service.to_string();
    let registered = service.clone();
    let url = url.to_string();
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
            runtime.block_on(run(service, url, out, erx, ready_tx, worker_outbox));
            runtime.shutdown_timeout(Duration::from_millis(100));
        })
        .map_err(|e| StartError::Unreachable(format!("Bus thread: {e}")))?;
    match ready_rx.recv() {
        Ok(Ok(client)) => Ok((
            BusHandle {
                tx: etx,
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

/// The delivery channel and the host's wake-up.
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

async fn run(
    service: String,
    url: String,
    out: Out,
    mut erx: tokio::sync::mpsc::UnboundedReceiver<WorkerCommand>,
    ready: std::sync::mpsc::Sender<Result<Arc<SupervisedClient>, StartError>>,
    outbox: Arc<citizen::outbox::Outbox>,
) {
    let options = SupervisedClient::connect_options(&service, &url)
        .fatal_on_registration_rejection(true)
        .bounded_incoming(64);
    let client = match tokio::time::timeout(CONNECT_TIMEOUT, options.connect()).await {
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
    let Some(mut incoming) = client.incoming_bounded() else {
        let _ = ready.send(Err(StartError::Unreachable("no incoming channel".into())));
        return;
    };
    let mut state = client.subscribe_state();
    let _ = ready.send(Ok(Arc::clone(&client)));
    citizen::macros::install("ced", &service, outbox.clone());
    citizen::outbox::spawn_publisher(client.clone(), outbox);

    let mut commands: HashMap<u64, IncomingCommand> = HashMap::new();
    let mut replies = tokio::task::JoinSet::new();
    let mut next_command = 0u64;
    let mut macro_jobs = citizen::macros::Jobs::default();
    let mut session = None;
    loop {
        tokio::select! {
            result = replies.join_next(), if !replies.is_empty() => {
                if let Some(Ok(Err(error))) = result {
                    tracing::warn!(%error, "ced Bus reply failed");
                }
            },
            cmd = incoming.recv() => {
                let cmd = match cmd {
                    Some(BoundedIncomingEvent::Command(cmd)) => cmd,
                    Some(BoundedIncomingEvent::Overflow { .. }) => {
                        // Frames were lost: mirrors reconcile as on a reconnect.
                        out.send(Delivery::Incoming(Incoming::Connection { up: true }));
                        continue;
                    }
                    None => break,
                };
                if let Some(topic) = cmd.topic() {
                    out.send(Delivery::Incoming(Incoming::Topic {
                        topic: topic.to_string(),
                        body: cmd.body.clone(),
                    }));
                    continue;
                }
                if cmd.command.is_empty() {
                    continue;
                }
                citizen::debug::received(&service, &cmd.command);
                if let Some((rc, body)) = citizen::debug::request_for(&service, "ced", &cmd.command, &cmd.body) {
                    citizen::debug::answered(&service, &cmd.command, rc);
                    let _ = tokio::time::timeout(Duration::from_secs(2), client.respond(&cmd, rc, &body.to_string())).await;
                    continue;
                }
                if citizen::macros::dispatch("ced", &cmd, client.clone(), &mut macro_jobs) { continue; }
                next_command += 1;
                let delivery = Delivery::Command(BusCommand {
                    id: next_command,
                    command: cmd.command.clone(),
                    body: if cmd.body.trim().is_empty() { "{}".to_string() } else { cmd.body.clone() },
                    caller_key: caller_key(&cmd),
                });
                if cmd.id.is_some() {
                    commands.insert(next_command, cmd);
                }
                out.send(delivery);
            }
            command = erx.recv() => {
                let effect = match command {
                    None => break,
                    Some(WorkerCommand::Shutdown(writer)) => {
                        session = writer;
                        break;
                    }
                    Some(WorkerCommand::Look(read)) => {
                        let (c, o) = (client.clone(), out.clone());
                        tokio::spawn(async move {
                            let headers = BTreeMap::new();
                            let body = read.body.to_string();
                            let call = c.call_with_headers_raw(read.service, read.command, &headers, &body);
                            let reply = match tokio::time::timeout(LOOK_BUDGET, call).await {
                                Ok(Ok(answer)) => Some(answer),
                                _ => None,
                            };
                            o.send(Delivery::Look { ticket: read.ticket, reply });
                        });
                        continue;
                    }
                    Some(WorkerCommand::LookRetry { delay, generation }) => {
                        let o = out.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(delay).await;
                            o.send(Delivery::LookRetry { generation });
                        });
                        continue;
                    }
                    Some(WorkerCommand::Effect(effect)) => effect,
                };
                match effect {
                    Effect::Notify { text, level } => {
                        let (c, o) = (client.clone(), out.clone());
                        tokio::spawn(async move {
                            let body = serde_json::json!({
                                "summary": "Ced", "body": text.chars().take(500).collect::<String>(),
                                "app": "ced", "level": if level == editor_model::types::Level::Error { "error" } else { "warning" },
                                "timeout_ms": 6000
                            }).to_string();
                            let headers = BTreeMap::new();
                            let result = tokio::time::timeout(Duration::from_secs(3),
                                c.call_with_headers_raw("notify", "notify.send", &headers, &body)).await;
                            if !matches!(result, Ok(Ok((0..=9, _, _)))) {
                                o.send(Delivery::NotificationFailed { text });
                            }
                        });
                    }
                    Effect::SaveMail { tab, path, text, expected, send, command } => {
                        let o = out.clone();
                        tokio::spawn(async move {
                            let (p, t) = (path.clone(), text.clone());
                            let result = tokio::task::spawn_blocking(move || crate::mail::save(&p, &t, expected.as_deref())).await.unwrap_or_else(|error| Err(error.to_string()));
                            o.send(Delivery::MailSaved { tab, path, text, send, command, result });
                        });
                    }
                    Effect::SendMail { tab, path, command } => {
                        let (c, o) = (client.clone(), out.clone());
                        tokio::spawn(async move {
                            let result = crate::mail::send(c.as_ref(), &path).await;
                            o.send(Delivery::MailSent { tab, command, result });
                        });
                    }
                    Effect::Send { req, out: request } => {
                        let (c, o) = (client.clone(), out.clone());
                        tokio::spawn(async move {
                            let deadline = tokio::time::Instant::now()
                                + Duration::from_millis(request.deadline_ms);
                            let headers = BTreeMap::new();
                            let call = c.call_with_headers_raw(
                                EDIT,
                                &request.command,
                                &headers,
                                &request.body,
                            );
                            let incoming = match tokio::time::timeout_at(deadline, call).await {
                                // A large success body (a snapshot page) is
                                // parsed here, off the UI thread.
                                Ok(Ok((rc, body, _))) if rc < 10 && body.len() > PARSE_OFF_UI_BYTES => {
                                    match serde_json::from_str::<serde_json::Value>(&body) {
                                        Ok(v) => Incoming::Parsed { req, rc, body: ParsedBody(v) },
                                        Err(_) => Incoming::Reply { req, rc, body },
                                    }
                                }
                                Ok(Ok((rc, body, error))) => Incoming::Reply {
                                    req, rc, body: failure_body(rc, body, error.as_deref(), &request.command)
                                },
                                Ok(Err(_)) => {
                                    tokio::time::sleep_until(deadline).await;
                                    Incoming::Deadline { req }
                                }
                                Err(_) => Incoming::Deadline { req },
                            };
                            o.send(Delivery::Incoming(incoming));
                        });
                    }
                    Effect::Respond { id, rc, body } => {
                        if let Some(cmd) = commands.remove(&id) {
                            citizen::debug::answered(&service, &cmd.command, rc);
                            let c = client.clone();
                            replies.spawn(async move {
                                tokio::time::timeout(Duration::from_secs(2), c.respond(&cmd, rc, &body))
                                    .await
                                    .map_err(|_| "Bus reply timed out".to_owned())?
                                    .map_err(|error| format!("Bus reply: {error}"))
                            });
                        }
                    }
                    Effect::Timer { id, ms } => {
                        let o = out.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(ms)).await;
                            o.send(Delivery::Incoming(Incoming::Timer { id }));
                        });
                    }
                    Effect::Subscribe { topic } => {
                        let (c, o) = (client.clone(), out.clone());
                        tokio::spawn(async move {
                            // The client replays only topics that once
                            // subscribed, so a failed first subscribe would
                            // leave ced deaf for good: retry with backoff,
                            // and once it lands treat it as a reconnect, so
                            // every mirror recovers what it missed.
                            let mut delay = Duration::from_millis(250);
                            let mut failed = false;
                            loop {
                                if matches!(c.state(), ConnState::ShuttingDown | ConnState::Fatal) {
                                    return;
                                }
                                match c.subscribe_topic(&topic).await {
                                    Ok(_) => break,
                                    Err(e) => {
                                        if !failed {
                                            tracing::warn!("subscribe {topic}: {e}; retrying");
                                        }
                                        failed = true;
                                        tokio::time::sleep(delay).await;
                                        delay = (delay * 2).min(Duration::from_secs(5));
                                    }
                                }
                            }
                            if failed {
                                o.send(Delivery::Incoming(Incoming::Connection { up: true }));
                            }
                            o.send(Delivery::Subscribed { topic });
                        });
                    }
                    Effect::Quit => break,
                    _ => {}
                }
            }
            changed = state.changed() => {
                if changed.is_err() {
                    break;
                }
                let now = *state.borrow_and_update();
                match now {
                    ConnState::Connected => {
                        out.send(Delivery::Incoming(Incoming::Connection { up: true }));
                    }
                    ConnState::Disconnected => {
                        out.send(Delivery::Incoming(Incoming::Connection { up: false }));
                    }
                    ConnState::ShuttingDown | ConnState::Fatal => break,
                    ConnState::Connecting => {}
                }
            }
        }
    }
    let deadline = std::time::Instant::now() + DRAIN;
    let until = tokio::time::Instant::from_std(deadline);
    let mut faults = Vec::new();
    // Replies already queued (a quit's own answer among them) go out
    // first, while ordinary replies are still allowed.
    drain_replies(&mut replies, until, &mut faults).await;
    // Then leave the Bus, so nothing new is routed here; the connection
    // stays up for the refusals below.
    match tokio::time::timeout_at(until, client.deregister_for_drain()).await {
        Ok(Ok(())) | Ok(Err(SupervisedError::Disconnected)) => {}
        Ok(Err(error)) => faults.push(format!("Bus deregister: {error}")),
        Err(_) => faults.push("Bus deregister timed out".into()),
    }
    // Every command not yet answered (delivered to the window, or still
    // queued here) is refused: no caller is left waiting on a closed ced.
    // Past the deregister only the shutdown reply path is open.
    while let Some(Some(event)) = incoming.recv().now_or_never() {
        if let BoundedIncomingEvent::Command(cmd) = event
            && cmd.topic().is_none()
            && !cmd.command.is_empty()
            && cmd.id.is_some()
        {
            next_command += 1;
            commands.insert(next_command, cmd);
        }
    }
    let closing = r#"{"error_code":"BUSY","message":"ced is closing"}"#;
    for (_, cmd) in commands.drain() {
        let c = client.clone();
        replies.spawn(async move {
            let refuse = c.respond_parts_shutdown_synth(
                cmd.generation,
                &cmd.from,
                &cmd.command,
                cmd.id.as_deref(),
                10,
                closing,
            );
            tokio::time::timeout(Duration::from_secs(2), refuse)
                .await
                .map_err(|_| "Bus reply timed out".to_owned())?
                .map_err(|error| format!("Bus reply: {error}"))
        });
    }
    drain_replies(&mut replies, until, &mut faults).await;
    if let Some(mut writer) = session {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let drained = tokio::task::spawn_blocking(move || writer.flush_for(remaining));
        match tokio::time::timeout_at(until, drained).await {
            Ok(Ok(true)) => {}
            _ => faults.push("session drain timed out or failed".into()),
        }
    }
    macro_jobs.finish().await;
    if tokio::time::timeout_at(until, client.close())
        .await
        .is_err()
    {
        faults.push("Bus close timed out".into());
    }
    out.send(Delivery::Stopped { faults });
}

/// Wait for the reply tasks, until `until`; what failed goes to `faults`.
async fn drain_replies(
    replies: &mut tokio::task::JoinSet<Result<(), String>>,
    until: tokio::time::Instant,
    faults: &mut Vec<String>,
) {
    while !replies.is_empty() {
        match tokio::time::timeout_at(until, replies.join_next()).await {
            Ok(Some(Ok(Ok(())))) => {}
            Ok(Some(Ok(Err(error)))) => faults.push(error),
            Ok(Some(Err(error))) => faults.push(format!("Bus reply: {error}")),
            Ok(None) => break,
            Err(_) => {
                faults.push("Bus reply drain timed out".into());
                replies.abort_all();
                break;
            }
        }
    }
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

/// Whether an instance answers `ced.ping` on `service` within 500 ms.
pub fn probe_running(url: &str, service: &str) -> bool {
    matches!(
        anonymous_call(
            url,
            service,
            "ced.ping",
            &serde_json::json!({}),
            PROBE_TIMEOUT
        ),
        Some((0, _))
    )
}

/// Ask the running instance to open `paths` (`ced.open`).
pub fn forward_open(url: &str, service: &str, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    match anonymous_call(
        url,
        service,
        "ced.open",
        &serde_json::json!({ "paths": paths }),
        Duration::from_secs(5),
    ) {
        Some((0, _)) => Ok(()),
        Some((rc, body)) => Err(format!("ced.open refused (rc {rc}): {body}")),
        None => Err(format!("no answer from {service}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_keeps_real_error_text() {
        let body = failure_body(10, "Permission denied".into(), None, "edit.open");
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["message"], "Permission denied");
        let body = failure_body(
            10,
            r#"{"error_code":"IO","message":"Disk full"}"#.into(),
            None,
            "edit.save",
        );
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["message"], "Disk full");
    }

    #[test]
    fn empty_broker_errors_have_a_code_command_and_sentence() {
        for body in ["", r#"{"error_code":"UNAVAILABLE","message":""}"#] {
            let body = failure_body(
                10,
                body.into(),
                Some("Service 'edit' not found"),
                "edit.open",
            );
            let value: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(value["error_code"], "UNAVAILABLE");
            assert!(
                value["message"]
                    .as_str()
                    .unwrap()
                    .contains("edit.open failed (UNAVAILABLE)")
            );
        }
    }

    fn cmd(from: &str, headers: &[(&str, &str)]) -> IncomingCommand {
        IncomingCommand {
            generation: 0,
            from: from.to_string(),
            command: "ced.ping".into(),
            id: Some("1".into()),
            args: serde_json::Value::Null,
            body: String::new(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn caller_keys_follow_the_edit_service_rules() {
        assert_eq!(
            caller_key(&cmd("ctl-90", &[("broker_origin", "local")])),
            "local:ctl-90"
        );
        assert_eq!(caller_key(&cmd("", &[("broker_origin", "local")])), "anon");
        assert_eq!(
            caller_key(&cmd(
                "x",
                &[
                    ("broker_origin", "mesh"),
                    ("broker_service", "svc"),
                    ("broker_peer", "beta")
                ]
            )),
            "mesh:svc@beta"
        );
        assert_eq!(caller_key(&cmd("x", &[])), "anon");
    }

    #[test]
    fn only_bus_effects_go_to_the_bus_thread() {
        assert!(is_bus(&Effect::Quit));
        assert!(is_bus(&Effect::Timer { id: 1, ms: 5 }));
        assert!(!is_bus(&Effect::SaveSession));
    }
}
