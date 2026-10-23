// SPDX-License-Identifier: MIT OR Apache-2.0
//! One lazy native Bus actor per process. No subprocesses or outcome polling.
use super::Finished;
use ::bus::native_client::{
    BoundedIncomingEvent, BoundedIncomingReceiver, ConnState, SupervisedClient,
};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, OnceLock, Weak, mpsc};
use std::time::{Duration, Instant};

pub(super) const RETENTION: Duration = Duration::from_secs(300);
const SEND_BUDGET: Duration = Duration::from_secs(2);
const PER_WINDOW: usize = 8;

struct Recipient {
    alive: Weak<()>,
    tx: mpsc::Sender<Finished>,
    ctx: egui::Context,
    viewport: egui::ViewportId,
}

impl Recipient {
    fn deliver(&self, finished: Finished) {
        if self.alive.upgrade().is_some() && self.tx.send(finished).is_ok() {
            self.ctx.request_repaint_of(self.viewport);
        }
    }
}

struct Record {
    window: u64,
    notification: Option<u64>,
    expires: Instant,
    recipient: Recipient,
    finished: Finished,
}

enum Message {
    Post { app: String, payload: serde_json::Value, record: Box<Record>, send_by: Instant },
    Closed(u64),
}

static CLIENT: OnceLock<Option<tokio::sync::mpsc::Sender<Message>>> = OnceLock::new();

pub(super) fn post(
    app: &str,
    window: u64,
    payload: serde_json::Value,
    finished: Finished,
    tx: mpsc::Sender<Finished>,
    alive: &Arc<()>,
    ctx: &egui::Context,
) {
    let recipient = Recipient { alive: Arc::downgrade(alive), tx, ctx: ctx.clone(), viewport: ctx.viewport_id() };
    let record = Record { window, notification: None, expires: Instant::now() + RETENTION, recipient, finished };
    let sender = CLIENT.get_or_init(|| {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        std::thread::Builder::new().name("macro-notifications".into()).spawn(move || {
            if let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                runtime.block_on(worker(rx));
                runtime.shutdown_timeout(Duration::from_millis(100));
            } else {
                // Closing the receiver returns queued posts to their fallback.
                let mut rx = rx;
                rx.close();
                while let Ok(message) = rx.try_recv() { fallback(message); }
            }
        }).ok().map(|_| tx)
    });
    let message = Message::Post { app: app.into(), payload, record: Box::new(record), send_by: Instant::now() + SEND_BUDGET };
    let Some(sender) = sender else { fallback(message); return; };
    if let Err(error) = sender.try_send(message) { fallback(error.into_inner()); }
}

pub(super) fn forget(window: u64) {
    if let Some(Some(sender)) = CLIENT.get() {
        // A full queue cannot keep a dead window alive: records also carry a
        // weak lifetime and are pruned on every actor event and deadline.
        let _ = sender.try_send(Message::Closed(window));
    }
}

fn fallback(message: Message) {
    if let Message::Post { record, .. } = message {
        record.recipient.deliver(record.finished);
    }
}

/// Transport-independent state, driven by replies, topic outcomes and timers.
#[derive(Default)]
struct Inbox {
    records: VecDeque<Record>,
    outstanding: BTreeMap<u64, (u64, Weak<()>, Instant)>,
    withdrawals: VecDeque<(u64, Instant)>,
}

impl Inbox {
    fn insert(&mut self, record: Record) {
        while self.outstanding.values().filter(|(window, _, _)| *window == record.window).count() >= PER_WINDOW {
            let id = *self.outstanding.iter().find(|(_, (window, _, _))| *window == record.window).unwrap().0;
            self.withdraw(id);
        }
        while self.records.iter().filter(|r| r.window == record.window).count() >= PER_WINDOW {
            let index = self.records.iter().position(|r| r.window == record.window).unwrap();
            self.records.remove(index);
        }
        self.records.push_back(record);
    }

    fn sent(&mut self, result: Option<u64>) {
        if let Some(id) = result {
            if let Some(record) = self.records.back_mut() {
                record.notification = Some(id);
                self.outstanding.insert(id, (record.window, record.recipient.alive.clone(), record.expires));
            }
        } else if let Some(record) = self.records.pop_back() {
            record.recipient.deliver(record.finished);
        }
    }

    fn outcome(&mut self, body: &serde_json::Value) {
        let Some(id) = body["id"].as_u64() else { return };
        if body["outcome"].is_null() { return; }
        self.outstanding.remove(&id);
        if let Some(index) = self.records.iter().position(|r| r.notification == Some(id)) {
            let record = self.records.remove(index).unwrap();
            if record.expires > Instant::now()
                && body["outcome"]["kind"] == "action" && body["outcome"]["action"] == "output" {
                record.recipient.deliver(record.finished);
            }
        }
    }

    fn withdraw(&mut self, id: u64) {
        if let Some((_, _, expires)) = self.outstanding.remove(&id) { self.withdrawals.push_back((id, expires)); }
        self.records.retain(|r| r.notification != Some(id));
    }
    fn closed(&mut self, window: u64) {
        let ids: Vec<_> = self.outstanding.iter().filter(|(_, (owner, _, _))| *owner == window).map(|(&id, _)| id).collect();
        for id in ids { self.withdraw(id); }
        self.records.retain(|r| r.window != window);
    }
    fn bus_error(&mut self) { self.records.retain(|r| r.notification.is_none()); }
    fn prune(&mut self, now: Instant) {
        self.records.retain(|r| r.expires > now && r.recipient.alive.upgrade().is_some());
        let ids: Vec<_> = self.outstanding.iter().filter(|(_, (_, alive, expires))| *expires <= now || alive.upgrade().is_none()).map(|(&id, _)| id).collect();
        for id in ids { self.withdraw(id); }
        self.withdrawals.retain(|(_, expires)| *expires > now);
    }
    fn next_expiry(&self) -> Option<Instant> {
        self.records.iter().map(|r| r.expires).chain(self.outstanding.values().map(|(_, _, expiry)| *expiry))
            .chain(self.withdrawals.iter().map(|(_, expiry)| *expiry)).min()
    }
}

struct Native {
    client: SupervisedClient,
    incoming: BoundedIncomingReceiver,
    state: tokio::sync::watch::Receiver<ConnState>,
}

impl Native {
    async fn connect(app: &str) -> Option<Self> {
        // Registration is required for topic delivery. The pid distinguishes
        // independent app processes without claiming the app's own Bus name.
        let service = format!("{app}-macros-{}", std::process::id());
        let url = ::bus::client_helpers::resolve_noded_url();
        let client = SupervisedClient::connect_options(&service, &url)
            .fatal_on_registration_rejection(true).bounded_incoming(128).connect().await.ok()?;
        let incoming = client.incoming_bounded()?;
        client.subscribe_topic("notify.changed").await.ok()?;
        let state = client.subscribe_state();
        Some(Self { client, incoming, state })
    }
}

async fn worker(mut rx: tokio::sync::mpsc::Receiver<Message>) {
    let mut native: Option<Native> = None;
    let mut inbox = Inbox::default();
    loop {
        inbox.prune(Instant::now());
        if let Some(native) = &native && native.client.is_connected()
            && let Some((id, _)) = inbox.withdrawals.pop_front() {
            // Best effort with a deadline; already-settled notifications may
            // refuse the close. There is no UI fallback for a withdrawal.
            let _ = tokio::time::timeout(SEND_BUDGET, native.client.call_with_headers_raw(
                "notify", "notify.close", &BTreeMap::new(), &serde_json::json!({"id": id}).to_string()
            )).await;
            continue;
        }
        let expires = inbox.next_expiry();
        let (incoming, state) = match native.as_mut() {
            Some(n) => (Some(&mut n.incoming), Some(&mut n.state)),
            None => (None, None),
        };
        tokio::select! {
            message = rx.recv() => match message {
                Some(Message::Post { app, payload, record, send_by }) => {
                    if record.recipient.alive.upgrade().is_none() || record.expires <= Instant::now() { continue; }
                    inbox.insert(*record);
                    let result = tokio::time::timeout_at(tokio::time::Instant::from_std(send_by), async {
                        if native.is_none() { native = Native::connect(&app).await; }
                        let client = &native.as_ref()?.client;
                        let (rc, body, _) = client.call_with_headers_raw("notify", "notify.send", &BTreeMap::new(), &payload.to_string()).await.ok()?;
                        if rc >= 10 { return None; }
                        serde_json::from_str::<serde_json::Value>(&body).ok()?["id"].as_u64()
                    }).await.ok().flatten();
                    // While the request is in flight, topic bodies queue in
                    // the bounded receiver. Bind its reply id before draining
                    // those bodies, including an immediate answer.
                    inbox.sent(result);
                }
                Some(Message::Closed(window)) => inbox.closed(window),
                None => break,
            },
            event = receive(incoming) => match event {
                Some(BoundedIncomingEvent::Command(cmd)) if cmd.topic() == Some("notify.changed") => {
                    if let Ok(body) = serde_json::from_str(&cmd.body) { inbox.outcome(&body); }
                }
                Some(BoundedIncomingEvent::Overflow { .. }) => inbox.bus_error(),
                None => { inbox.bus_error(); native = None; }
                _ => {}
            },
            state = changed(state) => {
                if state != Some(ConnState::Connected) { inbox.bus_error(); }
                if state.is_none() || matches!(state, Some(ConnState::Fatal | ConnState::ShuttingDown)) {
                    native = None;
                }
            },
            () = expire(expires) => inbox.prune(Instant::now()),
        }
    }
    if let Some(native) = native {
        let _ = tokio::time::timeout(SEND_BUDGET, native.client.close()).await;
    }
}

async fn receive(incoming: Option<&mut BoundedIncomingReceiver>) -> Option<BoundedIncomingEvent> {
    match incoming { Some(incoming) => incoming.recv().await, None => std::future::pending().await }
}

async fn changed(state: Option<&mut tokio::sync::watch::Receiver<ConnState>>) -> Option<ConnState> {
    match state {
        Some(state) => { state.changed().await.ok()?; Some(*state.borrow_and_update()) }
        None => std::future::pending().await,
    }
}

async fn expire(expires: Option<Instant>) {
    match expires {
        Some(expires) => tokio::time::sleep_until(tokio::time::Instant::from_std(expires)).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(state: &super::super::State, id: u64) -> Record {
        Record {
            window: state.window,
            notification: None,
            expires: Instant::now() + RETENTION,
            recipient: Recipient {
                alive: Arc::downgrade(&state.lifetime), tx: state.notify_tx.clone(),
                ctx: egui::Context::default(), viewport: egui::ViewportId::ROOT,
            },
            finished: Finished { title: format!("Run {id}"), output: format!("output {id}"), status: "done".into() },
        }
    }

    #[test]
    fn fake_bus_output_action_delivers_the_matching_run_only() {
        let mut state = super::super::State::new("ced");
        let mut inbox = Inbox::default();
        inbox.insert(record(&state, 1)); inbox.sent(Some(41));
        inbox.insert(record(&state, 2)); inbox.sent(Some(42));
        inbox.outcome(&serde_json::json!({"id": 41, "outcome": null}));
        inbox.outcome(&serde_json::json!({"id": 99, "outcome": {"action": "output"}}));
        assert!(state.notifications.try_recv().is_err());
        inbox.outcome(&serde_json::json!({"id": 42, "outcome": {"kind": "action", "action": "output"}}));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| state.show(&ctx));
        out.textures_delta.clear();
        assert!(state.editor_open);
        assert_eq!(state.retained.as_ref().unwrap().output, "output 2");
        assert_eq!(inbox.records.len(), 1);
    }

    #[test]
    fn fake_bus_other_outcomes_and_expiry_release_output() {
        let state = super::super::State::new("ced");
        for kind in ["dismissed", "closed", "expired", "other-action"] {
            let mut inbox = Inbox::default();
            inbox.insert(record(&state, 1)); inbox.sent(Some(41));
            inbox.outcome(&serde_json::json!({"id": 41, "outcome": {"kind": kind, "action": "other"}}));
            assert!(inbox.records.is_empty());
            assert!(state.notifications.try_recv().is_err());
        }
        let mut inbox = Inbox::default();
        inbox.insert(record(&state, 1)); inbox.sent(Some(41));
        inbox.prune(Instant::now() + RETENTION);
        assert!(inbox.records.is_empty());
        assert!(state.notifications.try_recv().is_err());
        let mut old = record(&state, 2);
        old.expires = Instant::now() - Duration::from_secs(1);
        inbox.insert(old); inbox.sent(Some(42));
        inbox.outcome(&serde_json::json!({"id": 42, "outcome": {"kind": "action", "action": "output"}}));
        assert!(inbox.records.is_empty());
        assert!(state.notifications.try_recv().is_err());
    }

    #[test]
    fn fake_bus_send_failure_opens_output_but_later_bus_error_never_does() {
        let mut state = super::super::State::new("ced");
        let mut inbox = Inbox::default();
        inbox.insert(record(&state, 1)); inbox.sent(None);
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| state.show(&ctx));
        out.textures_delta.clear();
        assert!(state.editor_open);
        assert_eq!(state.retained.as_ref().unwrap().output, "output 1");
        state.editor_open = false;
        inbox.insert(record(&state, 2)); inbox.sent(Some(42));
        inbox.bus_error();
        assert!(inbox.records.is_empty());
        assert!(state.notifications.try_recv().is_err());
        inbox.closed(state.window);
        assert_eq!(inbox.withdrawals.front().map(|(id, _)| *id), Some(42));
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| state.show(&ctx));
        out.textures_delta.clear();
        assert!(!state.editor_open);
    }

    #[test]
    fn retention_cap_and_window_close_are_per_window() {
        let first = super::super::State::new("ced");
        let second = super::super::State::new("ced");
        let mut inbox = Inbox::default();
        for n in 0..20 { inbox.insert(record(&first, n)); inbox.sent(Some(n)); }
        inbox.insert(record(&second, 30)); inbox.sent(Some(30));
        assert_eq!(inbox.records.len(), PER_WINDOW + 1);
        assert_eq!(inbox.records[0].notification, Some(12));
        inbox.closed(first.window);
        assert_eq!(inbox.records.len(), 1);
        assert_eq!(inbox.records[0].window, second.window);
        assert_eq!(inbox.withdrawals.iter().map(|(id, _)| *id).collect::<Vec<_>>(), (0..20).collect::<Vec<_>>());
        assert!(inbox.outstanding.contains_key(&30));
        drop(second);
        inbox.prune(Instant::now());
        assert!(inbox.records.is_empty());
        assert!(inbox.outstanding.is_empty());
        assert_eq!(inbox.withdrawals.back().map(|(id, _)| *id), Some(30));
    }
}
