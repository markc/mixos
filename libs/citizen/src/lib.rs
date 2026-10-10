// SPDX-License-Identifier: MIT OR Apache-2.0
//! citizen: one app's supervised native Bus connection. The app registers
//! under its service name, receives its commands as [`Delivery`]s, answers
//! each through its [`Handle`], calls other services, and on quit answers
//! every accepted command before the connection closes. Topics
//! (`theme.changed`, and service registration on `noded.props.changed`)
//! wake the app; nothing polls.
//!
//! Promoted from BusViewer's engine (`apps/busviewer/crates/inspector/src/bus.rs`)
//! when Prefs became its second owner (AGENTS.md §2.2). The only change is
//! that the app's name (`busviewer`, `prefs`) is a parameter: it names the
//! worker thread, the closing refusal and the `<app>.ping` / `<app>.show`
//! verbs used for single-instance activation.
use ::bus::native_client::{
    BoundedIncomingEvent, ConnState, IncomingCommand, NodedClient, SupervisedClient,
    SupervisedError,
};
use futures::channel::{mpsc, oneshot};
use futures::{FutureExt, SinkExt};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

/// What the connection hands the app.
#[derive(Debug, Clone)]
pub enum Delivery {
    /// A command for the app; answer it once with [`Handle::reply`].
    Command {
        id: u64,
        verb: String,
        body: String,
    },
    /// The set of registered services changed.
    Changed,
    /// The session theme changed.
    Theme,
    Connected,
    Disconnected,
}
#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub rc: u8,
    pub body: String,
    /// The broker's `error` header, when it refused the call itself
    /// (`Service 'x' not found`, overload): such refusals have no body.
    pub error: Option<String>,
}
enum Effect {
    Call(
        String,
        String,
        String,
        Duration,
        oneshot::Sender<Result<Reply, CallError>>,
    ),
}
#[derive(Debug, Clone)]
pub struct CallError {
    pub message: String,
    /// The call may have reached its target: its effect is unknown.
    pub outcome_unknown: bool,
}
impl CallError {
    fn not_sent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            outcome_unknown: false,
        }
    }
    fn transport(error: SupervisedError) -> Self {
        let outcome_unknown = !matches!(
            error,
            SupervisedError::Disconnected | SupervisedError::ShuttingDown
        );
        Self {
            message: error.to_string(),
            outcome_unknown,
        }
    }
}
impl From<String> for CallError {
    fn from(message: String) -> Self {
        Self {
            message,
            outcome_unknown: true,
        }
    }
}
impl From<&str> for CallError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for CallError {}
enum Control {
    Reply(u64, u8, Value),
    Quit,
}
#[derive(Clone)]
pub struct Handle {
    tx: tokio::sync::mpsc::Sender<Effect>,
    control: tokio::sync::mpsc::UnboundedSender<Control>,
    done: Arc<(Mutex<bool>, Condvar)>,
    /// What this handle was asked to do, in order (tests only).
    #[cfg(any(test, feature = "testing"))]
    log: Arc<Mutex<Vec<Logged>>>,
}

/// One thing a test handle was asked to do.
#[cfg(any(test, feature = "testing"))]
#[derive(Clone, Debug, PartialEq)]
pub enum Logged {
    Call {
        service: String,
        verb: String,
        body: String,
    },
    Reply {
        id: u64,
        rc: u8,
        body: Value,
    },
    Quit,
}

impl Handle {
    pub async fn raw(&self, service: &str, verb: &str, body: String) -> Result<Reply, CallError> {
        self.raw_within(service, verb, body, Duration::from_secs(CALL_SECONDS))
            .await
    }
    /// [`Handle::raw`] with its own time limit, for calls that legitimately
    /// run long (an install downloading); past it the outcome is unknown.
    pub async fn raw_within(
        &self,
        service: &str,
        verb: &str,
        body: String,
        limit: Duration,
    ) -> Result<Reply, CallError> {
        #[cfg(any(test, feature = "testing"))]
        self.log.lock().unwrap().push(Logged::Call {
            service: service.into(),
            verb: verb.into(),
            body: body.clone(),
        });
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Effect::Call(service.into(), verb.into(), body, limit, tx))
            .await
            .map_err(|_| CallError::not_sent("Bus stopped; no call sent"))?;
        rx.await
            .map_err(|_| CallError::from("Bus request abandoned"))?
    }
    pub async fn call(&self, service: &str, verb: &str, args: Value) -> Result<Reply, String> {
        self.raw(service, verb, args.to_string())
            .await
            .map_err(|e| e.to_string())
    }
    pub fn reply(&self, id: u64, rc: u8, body: Value) {
        #[cfg(any(test, feature = "testing"))]
        self.log.lock().unwrap().push(Logged::Reply {
            id,
            rc,
            body: body.clone(),
        });
        // Only the GUI sends replies, once per accepted command (at most 32).
        // This separate queue cannot lose a reply to outgoing call backpressure.
        // A closed receiver means the native connection has already ended.
        let _ = self.control.send(Control::Reply(id, rc, body));
    }
    pub fn quit(&self) {
        #[cfg(any(test, feature = "testing"))]
        self.log.lock().unwrap().push(Logged::Quit);
        // FIFO with replies: accepted replies are flushed before close.
        let _ = self.control.send(Control::Quit);
    }
    pub fn wait_done(&self) -> Result<(), String> {
        let (lock, changed) = &*self.done;
        let state = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 32 pending replies × their 2s budget, plus connection close.
        let (state, _) = changed
            .wait_timeout_while(state, Duration::from_secs(70), |done| !*done)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *state {
            Ok(())
        } else {
            Err("Bus shutdown did not complete".into())
        }
    }
    /// A handle with no connection: calls fail unsent, replies and the
    /// quit are only logged.
    #[cfg(any(test, feature = "testing"))]
    pub fn sink() -> Self {
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        let (control, _rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            tx,
            control,
            done: Arc::new((Mutex::new(true), Condvar::new())),
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }
    #[cfg(any(test, feature = "testing"))]
    pub fn log(&self) -> Vec<Logged> {
        self.log.lock().unwrap().clone()
    }
    #[cfg(any(test, feature = "testing"))]
    pub fn responses(&self) -> Vec<(u64, u8, Value)> {
        let replies = self.log().into_iter().filter_map(|l| match l {
            Logged::Reply { id, rc, body } => Some((id, rc, body)),
            _ => None,
        });
        replies.collect()
    }
    #[cfg(any(test, feature = "testing"))]
    pub fn has_quit(&self) -> bool {
        self.log().contains(&Logged::Quit)
    }
}

struct Finished(Arc<(Mutex<bool>, Condvar)>);
impl Drop for Finished {
    fn drop(&mut self) {
        let (lock, changed) = &*self.0;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        changed.notify_all();
    }
}

/// Register `service` on the broker at `url` for the app named `app`
/// (`busviewer`, `prefs`), and start delivering.
pub fn start(
    app: &str,
    service: &str,
    url: &str,
) -> Result<(Handle, mpsc::Receiver<Delivery>), String> {
    let (send, receive) = mpsc::channel(64);
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let (control, controls) = tokio::sync::mpsc::unbounded_channel();
    let (ready_send, ready_receive) = std::sync::mpsc::channel();
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let finished = done.clone();
    let service = service.to_owned();
    let url = url.to_owned();
    let closing = closing(app);
    std::thread::Builder::new()
        .name(format!("{app}-bus"))
        .spawn(move || {
            let _finished = Finished(finished);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => runtime.block_on(worker(
                    service, url, closing, send, rx, controls, ready_send,
                )),
                Err(error) => {
                    let _ = ready_send.send(Err(format!("Bus runtime: {error}")));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    ready_receive
        .recv_timeout(Duration::from_secs(15))
        .map_err(|e| format!("Bus startup: {e}"))??;
    Ok((
        Handle {
            tx,
            control,
            done,
            #[cfg(any(test, feature = "testing"))]
            log: Arc::new(Mutex::new(Vec::new())),
        },
        receive,
    ))
}

/// The answer every request gets once the app is closing.
fn closing(app: &str) -> String {
    json!({"error_code":"BUSY","message":format!("{app} is closing")}).to_string()
}

async fn worker(
    service: String,
    url: String,
    closing: String,
    mut send: mpsc::Sender<Delivery>,
    mut effects: tokio::sync::mpsc::Receiver<Effect>,
    mut controls: tokio::sync::mpsc::UnboundedReceiver<Control>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let connect = SupervisedClient::connect_options(&service, &url)
        .bounded_incoming(64)
        .fatal_on_registration_rejection(true)
        .connect();
    let client = match tokio::time::timeout(Duration::from_secs(5), connect).await {
        Ok(Ok(client)) => Arc::new(client),
        Ok(Err(error)) => {
            let _ = ready.send(Err(format!("Bus registration: {error}")));
            return;
        }
        Err(_) => {
            let _ = ready.send(Err("Bus registration timed out".into()));
            return;
        }
    };
    let Some(mut incoming) = client.incoming_bounded() else {
        let _ = ready.send(Err("no incoming Bus channel".into()));
        return;
    };
    let mut connection = client.subscribe_state();
    for topic in ["theme.changed".to_owned(), "noded.props.changed".to_owned()] {
        if !matches!(
            tokio::time::timeout(Duration::from_secs(2), client.subscribe_topic(&topic)).await,
            Ok(Ok(()))
        ) {
            let _ = ready.send(Err(format!("cannot subscribe to {topic}")));
            let _ = client.close().await;
            return;
        }
    }
    let _ = ready.send(Ok(()));
    let mut pending: HashMap<u64, IncomingCommand> = HashMap::new();
    let mut next_id = 0;
    let permits = Arc::new(tokio::sync::Semaphore::new(16));
    loop {
        tokio::select! {
            biased;
            control = controls.recv() => {
                match control {
                    Some(Control::Reply(id,rc,value)) => {
                        if let Some(command) = pending.remove(&id) {
                            let _ = tokio::time::timeout(Duration::from_secs(2),client.respond(&command,rc,&value.to_string())).await;
                        }
                    }
                    // Replies are FIFO with the quit, so whatever is still pending
                    // was never answered; nor was any request the client already
                    // buffered (the biased select takes the quit first). The
                    // broker never answers for a responder that disconnects, so
                    // admit nothing more, answer them all while the transport is
                    // still open, and close only once a drain finds none.
                    Some(Control::Quit) | None => {
                        let mut owed = unanswered(std::mem::take(&mut pending), || incoming.recv().now_or_never().flatten());
                        while !owed.is_empty() {
                            for command in &owed {
                                let _ = tokio::time::timeout(Duration::from_secs(2),client.respond(command,10,&closing)).await;
                            }
                            owed = unanswered(HashMap::new(), || incoming.recv().now_or_never().flatten());
                        }
                        break;
                    }
                }
            }
            command = incoming.recv() => {
                let command = match command {
                    Some(BoundedIncomingEvent::Command(command)) => command,
                    Some(BoundedIncomingEvent::Overflow{..}) => {
                        let _ = send.send(Delivery::Changed).await;
                        let _ = send.send(Delivery::Theme).await;
                        continue;
                    },
                    None => break,
                };
                if let Some(topic) = command.topic() {
                    if topic == "noded.props.changed" && command.headers.get("gap").is_none_or(|value|value != "true")
                        && serde_json::from_str::<Value>(&command.body).ok().is_some_and(|body|body["path"] != "services.registered") {
                        continue;
                    }
                    let _ = send.send(if topic == "theme.changed" { Delivery::Theme } else { Delivery::Changed }).await;
                    continue;
                }
                if command.command.is_empty() {
                    let _ = tokio::time::timeout(Duration::from_secs(2),client.respond(&command,10,"{\"error_code\":\"ARGUMENT\",\"message\":\"command verb is empty\"}")).await;
                    continue;
                }
                if pending.len() >= 32 {
                    let _ = tokio::time::timeout(Duration::from_secs(2),client.respond(&command,10,"{\"error_code\":\"BUSY\",\"message\":\"too many pending commands\"}")).await;
                    continue;
                }
                next_id += 1;
                let delivery = Delivery::Command {id:next_id,verb:command.command.clone(),body:if command.body.trim().is_empty(){"{}".into()}else{command.body.clone()}};
                pending.insert(next_id,command);
                let _ = send.send(delivery).await;
            }
            effect = effects.recv() => {
                let Some(effect) = effect else { break; };
                match effect {
                    Effect::Call(service,verb,body,limit,reply) => {
                        let Ok(permit) = permits.clone().try_acquire_owned() else {
                            let _ = reply.send(Err(CallError::not_sent("Bus call capacity exhausted; no call sent")));
                            continue;
                        };
                        let client = client.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            // The client enforces `limit`; the outer bound only guards a
                            // client that never answers at all.
                            let result = tokio::time::timeout(limit + Duration::from_secs(5),client.call_with_headers_raw_within(&service,&verb,&BTreeMap::new(),&body,limit)).await
                                .map_err(|_|CallError::from("Bus request timed out"))
                                .and_then(|v|v.map_err(CallError::transport))
                                .map(|(rc,body,error)|Reply{rc,body,error});
                            let _ = reply.send(result);
                        });
                    }
                }
            }
            changed = connection.changed() => {
                if changed.is_err() { break; }
                let event = match *connection.borrow_and_update() {
                    ConnState::Connected => Some(Delivery::Connected),
                    ConnState::Disconnected | ConnState::Connecting => { pending.clear(); Some(Delivery::Disconnected) },
                    ConnState::Fatal | ConnState::ShuttingDown => break,
                };
                if let Some(event) = event { let _ = send.send(event).await; }
            }
        }
    }
    let _ = send.try_send(Delivery::Disconnected);
    let _ = tokio::time::timeout(Duration::from_secs(2), client.close()).await;
}

/// How long one outgoing call may take, unless the caller says otherwise.
const CALL_SECONDS: u64 = 30;

/// The requests a closing worker still owes an answer: those `pending` (in
/// arrival order), then every one already buffered, taken with `next_ready`
/// until it has nothing ready. Topic deliveries expect no answer.
fn unanswered(
    pending: HashMap<u64, IncomingCommand>,
    mut next_ready: impl FnMut() -> Option<BoundedIncomingEvent>,
) -> Vec<IncomingCommand> {
    let mut pending: Vec<_> = pending.into_iter().collect();
    pending.sort_by_key(|(id, _)| *id);
    let mut owed: Vec<_> = pending.into_iter().map(|(_, command)| command).collect();
    while let Some(event) = next_ready() {
        if let BoundedIncomingEvent::Command(command) = event
            && command.topic().is_none()
        {
            owed.push(command);
        }
    }
    owed
}

fn anonymous(url: &str, service: &str, verb: &str, args: Value) -> Result<Reply, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        // Activation may arrive after registration but before the first map.
        let budget = if verb.ends_with(".show") { 20 } else { 5 };
        tokio::time::timeout(Duration::from_secs(budget), async {
            let client = NodedClient::connect_anonymous(url)
                .await
                .map_err(|e| e.to_string())?;
            let result = client
                .call_with_headers_raw(service, verb, &BTreeMap::new(), &args.to_string())
                .await;
            client.close().await;
            let (rc, body, error) = result.map_err(|e| e.to_string())?;
            Ok(Reply { rc, body, error })
        })
        .await
        .map_err(|_| "activation timed out".to_owned())?
    })
}

/// The broker URL this session's apps use (`MIXOS_NODED_URL`, else the
/// node configuration's default).
pub fn noded_url() -> String {
    ::bus::client_helpers::resolve_noded_url()
}

/// Is an instance of `app` already registered as `service`? (`<app>.ping`)
pub fn probe(url: &str, service: &str, app: &str) -> bool {
    anonymous(url, service, &format!("{app}.ping"), json!({})).is_ok_and(|r| r.rc == 0)
}

/// Ask the running instance to show its window (`<app>.show`).
pub fn forward(url: &str, service: &str, app: &str) -> Result<(), String> {
    let reply = anonymous(url, service, &format!("{app}.show"), json!({}))?;
    if reply.rc != 0 {
        Err(format!("activation refused: {}", reply.body))
    } else {
        Ok(())
    }
}

/// Restore and focus this process's window through compd (`comp`): wait for
/// it to map, find it by `app_id` and pid, restore it, then raise and focus it.
pub async fn show(handle: Handle, comp: String, app_id: &str) -> Result<Value, String> {
    let mapped = handle
        .call(
            &comp,
            "comp.window.wait",
            json!({"match":{"app_id":app_id},"until":"mapped","timeout_ms":10000}),
        )
        .await?;
    if mapped.rc != 0 {
        return Err(mapped.body);
    }
    let list = handle.call(&comp, "comp.windows.list", json!({})).await?;
    if list.rc != 0 {
        return Err(list.body);
    }
    let value: Value = serde_json::from_str(&list.body).map_err(|e| e.to_string())?;
    let window = value["windows"]
        .as_array()
        .and_then(|rows| {
            rows.iter().find(|w| {
                w["app_id"] == app_id && w["pid"].as_u64() == Some(u64::from(std::process::id()))
            })
        })
        .ok_or("window not known to compd")?;
    let mut target = json!({"id":window["id"],"generation":window["generation"]});
    let restored = handle
        .call(&comp, "comp.window.restore", target.clone())
        .await?;
    let state: Value = serde_json::from_str(&restored.body).map_err(|e| e.to_string())?;
    if restored.rc != 0 || state["minimized"] != false {
        return Err(restored.body);
    }
    target["raise"] = json!(true);
    let focused = handle.call(&comp, "comp.window.focus", target).await?;
    let state: Value = serde_json::from_str(&focused.body).map_err(|e| e.to_string())?;
    if focused.rc != 0 || state["focused"] != true {
        return Err(focused.body);
    }
    Ok(json!({"shown":true,"target":state}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replies_and_quit_survive_full_call_queue_in_order() {
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        let (control, mut controls) = tokio::sync::mpsc::unbounded_channel();
        let handle = Handle {
            tx,
            control,
            ..Handle::sink()
        };
        for _ in 0..64 {
            let (reply, _rx) = oneshot::channel();
            assert!(
                handle
                    .tx
                    .try_send(Effect::Call(
                        "example".into(),
                        "echo".into(),
                        String::new(),
                        Duration::from_secs(CALL_SECONDS),
                        reply
                    ))
                    .is_ok()
            );
        }
        handle.reply(42, 0, json!({"ok":true}));
        handle.quit();
        assert!(matches!(controls.try_recv(), Ok(Control::Reply(42, 0, _))));
        assert!(matches!(controls.try_recv(), Ok(Control::Quit)));
        assert!(handle.wait_done().is_ok());
    }
    #[test]
    fn unsent_calls_and_lost_replies_have_distinct_outcomes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime
            .block_on(Handle::sink().raw("example", "echo", String::new()))
            .unwrap_err();
        assert!(!error.outcome_unknown);
        assert!(!CallError::transport(SupervisedError::Disconnected).outcome_unknown);
        assert!(!CallError::transport(SupervisedError::ShuttingDown).outcome_unknown);
        assert!(CallError::from("lost response").outcome_unknown);
    }

    #[test]
    fn the_closing_refusal_names_the_app() {
        let refusal: Value = serde_json::from_str(&closing("prefs")).unwrap();
        assert_eq!(refusal["error_code"], "BUSY");
        assert_eq!(refusal["message"], "prefs is closing");
    }

    fn request(verb: &str, topic: Option<&str>) -> IncomingCommand {
        IncomingCommand {
            generation: 1,
            from: "caller".into(),
            command: verb.into(),
            id: Some(verb.into()),
            args: Value::Null,
            body: "{}".into(),
            headers: topic
                .map(|t| ("topic".to_owned(), t.to_owned()))
                .into_iter()
                .collect(),
        }
    }

    /// sol final pass (BusViewer): a quit that wins the race with buffered
    /// requests still owes each of them an answer, after the pending ones.
    #[test]
    fn a_closing_worker_owes_pending_and_buffered_requests() {
        let pending: HashMap<u64, IncomingCommand> = [
            (2, request("b.second", None)),
            (1, request("a.first", None)),
        ]
        .into();
        let mut buffered = vec![
            BoundedIncomingEvent::Command(request("c.buffered", None)),
            BoundedIncomingEvent::Overflow { dropped: 3 },
            BoundedIncomingEvent::Command(request("", Some("theme.changed"))),
            BoundedIncomingEvent::Command(request("d.buffered", None)),
        ]
        .into_iter();
        let owed = unanswered(pending, || buffered.next());
        let verbs: Vec<_> = owed.iter().map(|c| c.command.as_str()).collect();
        assert_eq!(verbs, ["a.first", "b.second", "c.buffered", "d.buffered"]);
        assert!(buffered.next().is_none(), "drained until nothing was ready");
        assert!(unanswered(HashMap::new(), || None).is_empty());
    }
}
