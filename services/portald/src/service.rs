// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ABP feed uses the shared settings Consumer for subscription, initial
//! reads, delivery loss, reconnect generations and bounded authority retries.
//! Each appearance read includes its atomic snapshot evidence from settingsd.
use crate::status::{self, Shared};
use bus::PortReply;
use bus::native_client::{
    BoundedIncomingEvent, BoundedIncomingReceiver, ConnState, IncomingCommand, SupervisedClient,
};
use serde_json::{Value, json};
use settings::appearance::AppearanceProjection;
use settings::consumer::{Consumer, Work, WorkKind};
use settings::native;
use settings::{Binding, Diagnostic, Snapshot};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub const VERBS: &[&str] = &["portald.status"];
pub const SOURCE: &str = "settingsd";
pub const FETCH_VERB: &str = "settings.appearance.get";
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(1);
const DEREGISTER_TIMEOUT: Duration = Duration::from_secs(3);

pub fn manifest() -> Vec<bus::VerbDescriptor> {
    std::iter::once(bus::VerbDescriptor::new(
        "HELP",
        &[],
        "List served verbs",
        true,
    ))
    .chain(
        VERBS
            .iter()
            .map(|verb| bus::VerbDescriptor::new(verb, &[], "Settings portal status", true)),
    )
    .collect()
}

fn fault(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("read_failed", FETCH_VERB, message)
}

struct Read {
    snapshot: Option<Snapshot>,
    appearance: Option<AppearanceProjection>,
}

/// One bounded atomic read. A projection cannot be forwarded without matching
/// binding and snapshot identity, and the Consumer validates the snapshot.
async fn fetch(client: &SupervisedClient, binding: &Binding) -> Result<Read, Diagnostic> {
    let reply = client
        .call_typed(SOURCE, FETCH_VERB, json!({ "binding": binding }))
        .await
        .map_err(|error| fault(error.to_string()))?;
    let value = match reply {
        PortReply::Ok { rc: 0, value } => value,
        PortReply::Ok { rc, value } => return Err(fault(format!("{FETCH_VERB} rc {rc}: {value}"))),
        PortReply::AppError { rc, message } => {
            return Err(fault(format!("{FETCH_VERB} rc {rc}: {message}")));
        }
    };
    if serde_json::to_vec(&value)
        .map_err(|e| fault(e.to_string()))?
        .len()
        > settings::MAX_SNAPSHOT_BYTES + 64 * 1024
    {
        return Err(fault("Appearance response exceeds envelope budget"));
    }
    if value.get("status").and_then(Value::as_str) != Some("current") {
        return Err(fault("Authority did not return a current projection"));
    }
    let appearance: AppearanceProjection = serde_json::from_value(
        value
            .get("appearance")
            .cloned()
            .ok_or_else(|| fault("Missing appearance"))?,
    )
    .map_err(|error| fault(error.to_string()))?;
    appearance.validate()?;
    let snapshot: Snapshot = serde_json::from_value(
        value
            .get("snapshot")
            .cloned()
            .ok_or_else(|| fault("Missing snapshot evidence"))?,
    )
    .map_err(|error| fault(error.to_string()))?;
    if &appearance.binding != binding || &snapshot.binding != binding {
        return Err(Diagnostic::new(
            "wrong_target",
            "binding",
            "Appearance binding differs",
        ));
    }
    let expected = AppearanceProjection::from_snapshot(&snapshot, appearance.accent)?;
    if expected != appearance {
        return Err(fault(
            "Appearance differs from its atomic snapshot evidence",
        ));
    }
    Ok(Read {
        snapshot: Some(snapshot),
        appearance: Some(appearance),
    })
}

async fn execute(
    client: Arc<SupervisedClient>,
    work: Work,
    fetch_timeout: Duration,
) -> Result<Read, Diagnostic> {
    if work.kind() == WorkKind::Subscribe {
        return native::execute(&client, &work).await.map(|snapshot| Read {
            snapshot,
            appearance: None,
        });
    }
    if native::live_generation(&client) != Some(work.generation()) {
        return Err(fault("Connection changed before appearance read"));
    }
    let result = tokio::time::timeout(fetch_timeout, fetch(&client, work.binding()))
        .await
        .map_err(|_| Diagnostic::new("read_timeout", FETCH_VERB, "Appearance read timed out"))?;
    if native::live_generation(&client) != Some(work.generation()) {
        return Err(fault("Connection changed during appearance read"));
    }
    result
}

struct Job {
    work: Work,
    future: Pin<Box<dyn Future<Output = Result<Read, Diagnostic>> + Send>>,
}

async fn retry_after(delay: Option<Duration>) {
    match delay {
        Some(delay) => tokio::time::sleep(delay).await,
        None => std::future::pending::<()>().await,
    }
}

/// The outcome of one read completion.
enum Settled {
    /// The read belonged to a superseded connection. Nothing was adopted.
    Stale,
    /// Completed on the live connection. `Some` is replacement work it scheduled.
    Completed(Option<Work>),
}

/// Settles a completion against the connection sampled just before it. A result from
/// a superseded connection is never completed. The consumer moves to the live
/// connection instead, and the stale result is discarded.
fn settle(
    consumer: &mut Consumer,
    work: &Work,
    live: Option<u64>,
    result: Result<Option<Snapshot>, Diagnostic>,
) -> Settled {
    if live != Some(work.generation()) {
        match live {
            Some(generation) => {
                consumer.connected(generation);
            }
            None => consumer.disconnected(),
        }
        return Settled::Stale;
    }
    Settled::Completed(consumer.complete(work, result))
}

/// At most one summary line is logged per this interval for each kind of event. The
/// first event is always logged at once. Counts in `portald.status` are never dropped.
pub const REFUSAL_REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// Rate-limits one warning. A peer or a full lane can produce events as fast as it
/// likes, so logging each one would turn hints into unbounded log writes.
#[derive(Default)]
struct ReportLimit {
    reported_at: Option<std::time::Instant>,
    /// Events counted since the last logged line.
    unreported: u64,
}

/// One line to log, covering `count` events since the previous line.
#[derive(Debug, PartialEq, Eq)]
struct Summary {
    count: u64,
}

impl ReportLimit {
    /// Constant time and allocation-free. Returns a summary only when a line is due:
    /// the first event, or the first after an interval has passed.
    fn record(&mut self, now: std::time::Instant, count: u64) -> Option<Summary> {
        self.unreported = self.unreported.saturating_add(count);
        let due = match self.reported_at {
            None => true,
            Some(at) => now.saturating_duration_since(at) >= REFUSAL_REPORT_INTERVAL,
        };
        if !due {
            return None;
        }
        self.reported_at = Some(now);
        Some(Summary {
            count: std::mem::take(&mut self.unreported),
        })
    }
}

/// A settings delivery on our topic that the broker did not stamp as settingsd's
/// is never decoded. It is counted exactly in `portald.status`, and logged with its
/// reason at a bounded rate, so the silence is visible. The portal keeps its last
/// accepted projection and does not guess.
fn refuse_delivery(status: &Shared, log: &mut ReportLimit, reason: &'static str) {
    let report = log.record(std::time::Instant::now(), 1);
    status::update(status, |s| {
        s.delivery_refusals += 1;
        s.last_delivery_refusal = Some(reason);
        if report.is_some() {
            s.refusal_reports += 1;
        }
    });
    if let Some(report) = report {
        tracing::warn!(
            refused = report.count,
            reason,
            "settings deliveries refused since the last report: the broker did not stamp \
             settingsd as the owner; missing_broker_service usually means noded predates \
             the settingsd topic reservation"
        );
    }
}

/// A full lane drops commands and says how many. That loss is a hint, never data:
/// the feed cannot tell a dropped settings delivery from any other command, so the
/// next authenticated read covers it. It is counted exactly and logged at a bounded rate.
fn lane_overflowed(status: &Shared, log: &mut ReportLimit, dropped: u64) {
    let report = log.record(std::time::Instant::now(), dropped);
    status::update(status, |s| {
        s.lane_dropped = s.lane_dropped.saturating_add(dropped);
        if report.is_some() {
            s.overflow_reports += 1;
        }
    });
    if let Some(report) = report {
        tracing::warn!(
            dropped = report.count,
            "settings lane overflowed: the feed was behind, so commands were dropped; \
             treated as a hint, and the next authenticated read covers any lost delivery"
        );
    }
}

/// A reply that cannot be queued is shed. The requester times out. Counted exactly.
fn reply_shed(status: &Shared, log: &mut ReportLimit) {
    let report = log.record(std::time::Instant::now(), 1);
    status::update(status, |s| {
        s.replies_dropped = s.replies_dropped.saturating_add(1);
        if report.is_some() {
            s.reply_reports += 1;
        }
    });
    if let Some(report) = report {
        tracing::warn!(
            shed = report.count,
            "portald replies shed: the reply queue is full; requesters will time out"
        );
    }
}

/// The spacing of hint-driven authority reads: at most one starts per interval,
/// counted from the start of the previous hint-driven read. A hint storm costs one
/// read per interval, never one read per hint.
const HINT_INTERVAL: Duration = Duration::from_millis(250);

/// Scheduling for hints, kept apart from the consumer's own work. A hint never
/// discards a read in flight. Hints that arrive during a read coalesce into one
/// `pending` flag, and at most one follow-up read starts per completion.
#[derive(Default)]
struct Hints {
    pending: bool,
    last_read: Option<tokio::time::Instant>,
    /// When the select loop should next call `tick`, if a hint is waiting out the interval.
    due: Option<tokio::time::Instant>,
}

impl Hints {
    fn hint(&mut self) {
        self.pending = true;
    }

    /// Starts the hint-driven read when one is wanted, nothing is in flight, the
    /// connection is live and the interval has passed. Otherwise it records the
    /// deadline at which the interval ends.
    fn tick(&mut self, consumer: &mut Consumer) {
        if !self.pending || consumer.generation().is_none() || consumer.current_work().is_some() {
            return;
        }
        let now = tokio::time::Instant::now();
        if let Some(last) = self.last_read
            && now < last + HINT_INTERVAL
        {
            self.due = Some(last + HINT_INTERVAL);
            return;
        }
        self.pending = false;
        self.due = None;
        self.last_read = Some(now);
        consumer.refresh();
    }
}

async fn hint_due(due: Option<tokio::time::Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending::<()>().await,
    }
}

/// Upper bound on replies queued by drains before they are sent. It matches the
/// drain bound, so one completion can queue at most one lane's worth of replies.
const REPLY_QUEUE_LIMIT: usize = 64;

/// A reply classified during a drain, sent later by `send_replies`. Only the send
/// awaits, and it never runs between a read's fence and its completion.
struct Reply {
    command: IncomingCommand,
    rc: u8,
    body: String,
}

/// Everything the feed schedules from its lane. The select arms and the completion
/// drain share it, so an event is classified the same way whichever path reaches it.
#[derive(Default)]
struct Lane {
    hints: Hints,
    refusals: ReportLimit,
    overflows: ReportLimit,
    reply_sheds: ReportLimit,
    replies: VecDeque<Reply>,
}

/// Sends the replies that classification queued. Runs at the top of the loop, after
/// any completion has been settled, so no await sits between a fence and `complete`.
async fn send_replies(client: &SupervisedClient, replies: &mut VecDeque<Reply>) {
    while let Some(reply) = replies.pop_front() {
        if let Err(error) = client.respond(&reply.command, reply.rc, &reply.body).await {
            tracing::warn!(%error, "portald reply unavailable");
        }
    }
}

/// Classifies one event from the bounded lane. Synchronous by design: the completion
/// drain calls it between a read's result and its completion, so it must not await.
/// Hints update the consumer and hint state at once. Replies are queued for
/// `send_replies`.
fn handle_event(
    status: &Shared,
    consumer: &mut Consumer,
    lane: &mut Lane,
    event: BoundedIncomingEvent,
) -> anyhow::Result<()> {
    match event {
        BoundedIncomingEvent::Overflow { dropped } => {
            // A dropped command may have been a settings delivery, and the lane cannot
            // say which. Treat the loss as a hint. Never call consumer.lost() here: that
            // would invalidate the read in flight, and a repeating overflow would
            // starve every authenticated read.
            lane_overflowed(status, &mut lane.overflows, dropped);
            lane.hints.hint();
            lane.hints.tick(consumer);
        }
        BoundedIncomingEvent::Command(command) => {
            if command.is_topic_delivery() {
                match native::Decoded::admit(consumer.binding(), &command) {
                    native::Admission::Admitted(delivery) => {
                        consumer.decoded_delivery(*delivery);
                        // Retained publications also wake a fresh projection read
                        // on settingsd restart, even at an unchanged revision.
                        consumer.refresh();
                    }
                    native::Admission::Refused(reason) => {
                        // A refused frame is a HINT, never data: its body is not
                        // decoded or adopted. It only asks for one authenticated
                        // settings.appearance.get. It never calls refresh() here,
                        // because refresh() during a read discards that read's
                        // result. Old nodeds (before the settingsd topic
                        // reservation) never stamp, so this read is how they
                        // still converge.
                        refuse_delivery(status, &mut lane.refusals, reason);
                        lane.hints.hint();
                        lane.hints.tick(consumer);
                    }
                    native::Admission::Other => {}
                }
                return Ok(());
            }
            let (rc, value) = match command.command.as_str() {
                "HELP" => (0, json!(manifest())),
                "portald.status" => {
                    let mut report = status::report(status);
                    report["settings_generation"] = json!(consumer.generation());
                    (0, report)
                }
                other => (10, json!({"status": "not_served", "verb": other})),
            };
            let body = serde_json::to_string(&value)?;
            if lane.replies.len() == REPLY_QUEUE_LIMIT {
                reply_shed(status, &mut lane.reply_sheds);
            } else {
                lane.replies.push_back(Reply { command, rc, body });
            }
        }
    }
    Ok(())
}

/// Where the feed reads its commands. Production reads the client's bounded
/// receiver; the feed tests add injected frames beside the real broker traffic.
pub trait CommandSource: Send {
    fn next_event(&mut self) -> impl Future<Output = Option<BoundedIncomingEvent>> + Send;
    /// The next event that is already queued, without waiting.
    fn try_next_event(&mut self) -> Option<BoundedIncomingEvent>;
}

impl CommandSource for BoundedIncomingReceiver {
    fn next_event(&mut self) -> impl Future<Output = Option<BoundedIncomingEvent>> + Send {
        self.recv()
    }

    fn try_next_event(&mut self) -> Option<BoundedIncomingEvent> {
        self.try_recv()
    }
}

/// Upper bound on events drained before a read completion is handled. It equals
/// the bounded lane's capacity, so one drain can never be longer than a full queue.
const DRAIN_LIMIT: usize = 64;

/// Starts portald's bus client. Lazy start never exhausts an initial attempt
/// budget: the native supervisor keeps retrying transport with capped backoff
/// until the feed is closed.
pub fn connect(
    noded_url: &str,
) -> anyhow::Result<(Arc<SupervisedClient>, BoundedIncomingReceiver)> {
    let build = buildinfo::build_info!();
    let provenance = bus::RegisterProvenance::from_parts(
        build.pkg,
        build.version,
        build.git_sha,
        build.git_dirty,
        build.build_time,
        buildinfo::now_rfc3339(),
    );
    let client = Arc::new(
        SupervisedClient::connect_options("portald", noded_url)
            .bounded_incoming(64)
            .fatal_on_registration_rejection(true)
            .with_verbs(manifest())
            .with_provenance(provenance)
            .start(),
    );
    let incoming = client
        .incoming_bounded()
        .ok_or_else(|| anyhow::anyhow!("native incoming already taken"))?;
    Ok((client, incoming))
}

pub async fn feed(
    binding: Binding,
    noded_url: String,
    projections: mpsc::Sender<AppearanceProjection>,
    status: Shared,
) -> anyhow::Result<()> {
    let (client, incoming) = connect(&noded_url)?;
    run(binding, client, incoming, projections, status).await
}

/// The feed loop over an already connected client. It owns the client: it
/// deregisters and closes it before returning.
pub async fn run<I: CommandSource>(
    binding: Binding,
    client: Arc<SupervisedClient>,
    incoming: I,
    projections: mpsc::Sender<AppearanceProjection>,
    status: Shared,
) -> anyhow::Result<()> {
    run_with_fetch_timeout(
        binding,
        client,
        incoming,
        projections,
        status,
        FETCH_TIMEOUT,
    )
    .await
}

/// `run` with the appearance read's timeout supplied. The feed tests hold a read
/// open while they flood the lane, so they need a timeout that a loaded host
/// cannot trip. Production always uses `run`, which uses `FETCH_TIMEOUT`.
pub async fn run_with_fetch_timeout<I: CommandSource>(
    binding: Binding,
    client: Arc<SupervisedClient>,
    mut incoming: I,
    projections: mpsc::Sender<AppearanceProjection>,
    status: Shared,
    fetch_timeout: Duration,
) -> anyhow::Result<()> {
    let mut consumer = match Consumer::for_shell(binding) {
        Ok(consumer) => consumer,
        Err(error) => {
            client.close().await;
            anyhow::bail!(error.message);
        }
    };
    let mut state = client.subscribe_state();
    let mut job: Option<Job> = None;
    let mut lane = Lane::default();
    let result = loop {
        // Replies classified by a drain go out here, before any state is sampled.
        send_replies(&client, &mut lane.replies).await;
        // Sample INITIAL state and every generation before waiting for edges.
        // Consumer::connected deduplicates already processed generations.
        let now = *state.borrow_and_update();
        if now == ConnState::Fatal {
            break Err(anyhow::anyhow!("portald registration rejected"));
        }
        match native::live_generation(&client) {
            Some(generation) => {
                consumer.connected(generation);
            }
            None if consumer.generation().is_some() => consumer.disconnected(),
            None => {}
        }
        // Replacing this future cancels superseded work without spawning another
        // worker. Status requests and lifecycle events stay responsive during reads.
        if job.as_ref().map(|job| &job.work) != consumer.current_work() {
            job = consumer.current_work().cloned().map(|work| {
                if work.kind() == WorkKind::Read {
                    status::update(&status, |s| s.authority_reads += 1);
                }
                Job {
                    future: Box::pin(execute(client.clone(), work.clone(), fetch_timeout)),
                    work,
                }
            });
        }
        let retry = consumer.retry_delay();
        // Unbiased on purpose. Fairness is not what orders a hint before a
        // completion: the completion arm drains the queue itself before it
        // completes. A biased event-first select would starve completions under
        // a sustained event stream.
        tokio::select! {
            event = incoming.next_event() => {
                match event {
                    None => break Ok(()),
                    Some(event) => handle_event(&status, &mut consumer, &mut lane, event)?,
                }
            }
            _ = projections.closed() => break Ok(()),
            changed = state.changed() => {
                if changed.is_err() { break Ok(()); }
            }
            _ = retry_after(retry) => { consumer.retry(); }
            result = async { job.as_mut().expect("enabled job").future.as_mut().await }, if job.is_some() => {
                let completed = job.take().expect("completed job");
                let (snapshot, appearance) = match result {
                    Ok(read) => (Ok(read.snapshot), read.appearance),
                    Err(error) => (Err(error), None),
                };
                // Queued events count before this completion schedules anything. A
                // hint already in the lane is part of the same burst, so it must not
                // cost a second read. The drain is bounded, and it never awaits, so a
                // sustained stream cannot hold the completion forever.
                for _ in 0..DRAIN_LIMIT {
                    let Some(event) = incoming.try_next_event() else { break };
                    handle_event(&status, &mut consumer, &mut lane, event)?;
                }
                // No await since the drain: the connection is sampled immediately before
                // the completion it fences.
                let live = native::live_generation(&client);
                match settle(&mut consumer, &completed.work, live, snapshot) {
                    Settled::Stale => {}
                    Settled::Completed(replacement) => {
                        // When completion scheduled replacement work, this result is not the
                        // latest authority state. Forwarding it, or refreshing on mismatch,
                        // would only queue more reads for the same revision.
                        if replacement.is_none()
                            && let Some(appearance) = appearance
                            && consumer.is_confirmed()
                        {
                            let matches = consumer.current().is_some_and(|snapshot| {
                                appearance.identity() == (snapshot.incarnation.as_str(), snapshot.revision, snapshot.design_revision)
                            });
                            if matches {
                                if projections.send(appearance).await.is_err() { break Ok(()); }
                                if let Some(update) = consumer.pending().cloned() {
                                    consumer.acknowledge(&update);
                                }
                            } else {
                                // A delivery overtook this read. Fetch the new atomic projection.
                                consumer.refresh();
                            }
                        }
                        if let Some(error) = consumer.fault() {
                            status::update(&status, |s| s.settings_fault = Some(error.message.clone()));
                        }
                        lane.hints.tick(&mut consumer);
                    }
                }
            }
            _ = hint_due(lane.hints.due), if lane.hints.due.is_some() => {
                lane.hints.due = None;
                lane.hints.tick(&mut consumer);
            }
        }
    };
    let _ = tokio::time::timeout(DEREGISTER_TIMEOUT, client.deregister()).await;
    client.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn the_manifest_is_read_only() {
        let manifest = manifest();
        assert!(manifest.iter().all(|verb| verb.read_only));
        assert!(manifest.iter().any(|verb| verb.name == "portald.status"));
    }

    #[test]
    fn hints_during_a_read_merge_into_one_follow_up() {
        let binding = Binding {
            instance: "host".into(),
            profile: "default".into(),
        };
        let mut consumer = Consumer::for_shell(binding.clone()).unwrap();
        let mut hints = Hints::default();
        let subscribe = consumer.connected(1).unwrap();
        let read = consumer.complete(&subscribe, Ok(None)).unwrap();
        // A burst during the in-flight read discards nothing and starts nothing.
        for _ in 0..3 {
            hints.hint();
            hints.tick(&mut consumer);
        }
        assert!(consumer.current_work() == Some(&read));
        // The read completes. The first tick after it starts exactly one follow-up.
        let snapshot = Snapshot {
            schema: settings::SCHEMA,
            binding,
            incarnation: "a".into(),
            revision: settings::Revision(2),
            design_revision: settings::Revision(1),
            source_digest: "source".into(),
            desktop: settings::Desktop::default(),
            effective: settings::resolve(&settings::Desktop::default()).unwrap(),
        };
        assert!(consumer.complete(&read, Ok(Some(snapshot))).is_none());
        hints.tick(&mut consumer);
        let follow_up = consumer.current_work().cloned().unwrap();
        assert_eq!(follow_up.kind(), WorkKind::Read);
        assert!(!hints.pending);
        // Hints during the follow-up wait for it: no second read is in flight.
        hints.hint();
        hints.tick(&mut consumer);
        assert!(consumer.current_work() == Some(&follow_up));
        assert!(hints.pending);
    }

    #[test]
    fn a_refused_settings_delivery_is_counted_with_its_reason() {
        let shared = status::shared();
        let mut log = ReportLimit::default();
        refuse_delivery(&shared, &mut log, native::REFUSED_MISSING_OWNER);
        refuse_delivery(&shared, &mut log, native::REFUSED_WRONG_OWNER);
        let report = status::report(&shared);
        assert_eq!(report["portal"]["delivery_refusals"], 2);
        assert_eq!(
            report["portal"]["last_delivery_refusal"],
            native::REFUSED_WRONG_OWNER
        );
    }

    #[test]
    fn a_flood_of_refusals_logs_once_then_once_per_interval() {
        let start = std::time::Instant::now();
        let mut log = ReportLimit::default();
        // The first refusal is logged at once. The rest of a same-instant flood is
        // counted but not logged.
        assert_eq!(log.record(start, 1), Some(Summary { count: 1 }));
        let flood = 100_000;
        let logged: Vec<_> = (0..flood).filter_map(|_| log.record(start, 1)).collect();
        assert!(logged.is_empty(), "same-instant refusals must not log");
        // Ten seconds on, one summary covers everything counted since the first line.
        let later = start + REFUSAL_REPORT_INTERVAL;
        assert_eq!(log.record(later, 1), Some(Summary { count: flood + 1 }));
        // Still inside the next interval: nothing is logged, however many arrive.
        let within = later + REFUSAL_REPORT_INTERVAL / 2;
        assert!((0..flood).all(|_| log.record(within, 1).is_none()));
    }

    #[test]
    fn a_flood_of_lane_overflows_logs_a_bounded_number_of_lines() {
        let start = std::time::Instant::now();
        let mut log = ReportLimit::default();
        // Each overflow marker carries the loss since the previous marker. A storm of
        // markers, one per simulated drain pass, logs at most once per interval.
        let logged: Vec<_> = (0..100_000u64)
            .map(|pass| start + Duration::from_micros(pass))
            .filter_map(|now| log.record(now, 136))
            .collect();
        assert_eq!(logged, vec![Summary { count: 136 }]);
    }

    #[test]
    fn an_overflow_is_a_hint_and_never_discards_the_read_in_flight() {
        let binding = Binding {
            instance: "host".into(),
            profile: "default".into(),
        };
        let shared = status::shared();
        let mut consumer = Consumer::for_shell(binding.clone()).unwrap();
        let mut lane = Lane::default();
        let subscribe = consumer.connected(1).unwrap();
        let read = consumer.complete(&subscribe, Ok(None)).unwrap();
        // A lane overflow during the read is a hint: the read stays in flight and
        // the follow-up is only wanted.
        for _ in 0..50 {
            handle_event(
                &shared,
                &mut consumer,
                &mut lane,
                BoundedIncomingEvent::Overflow { dropped: 3 },
            )
            .unwrap();
        }
        assert!(consumer.current_work() == Some(&read));
        assert!(lane.hints.pending);
        let snapshot = Snapshot {
            schema: settings::SCHEMA,
            binding,
            incarnation: "a".into(),
            revision: settings::Revision(1),
            design_revision: settings::Revision(1),
            source_digest: "source".into(),
            desktop: settings::Desktop::default(),
            effective: settings::resolve(&settings::Desktop::default()).unwrap(),
        };
        // The read completes with its own result. It is adopted, not replaced.
        let settled = settle(&mut consumer, &read, Some(1), Ok(Some(snapshot)));
        assert!(matches!(settled, Settled::Completed(None)));
        assert_eq!(
            consumer.current().map(|s| s.revision),
            Some(settings::Revision(1))
        );
        assert_eq!(status::report(&shared)["portal"]["lane_dropped"], 150);
    }

    #[test]
    fn a_read_from_a_superseded_connection_is_never_adopted() {
        let binding = Binding {
            instance: "host".into(),
            profile: "default".into(),
        };
        let mut consumer = Consumer::for_shell(binding.clone()).unwrap();
        let subscribe = consumer.connected(1).unwrap();
        let read = consumer.complete(&subscribe, Ok(None)).unwrap();
        let snapshot = Snapshot {
            schema: settings::SCHEMA,
            binding,
            incarnation: "a".into(),
            revision: settings::Revision(2),
            design_revision: settings::Revision(1),
            source_digest: "source".into(),
            desktop: settings::Desktop::default(),
            effective: settings::resolve(&settings::Desktop::default()).unwrap(),
        };
        // The connection moved to generation 2 before the completion. The consumer
        // still holds generation 1, so without the fence it would accept this result.
        let settled = settle(&mut consumer, &read, Some(2), Ok(Some(snapshot.clone())));
        assert!(matches!(settled, Settled::Stale));
        assert!(consumer.current().is_none(), "stale result adopted");
        assert_eq!(consumer.generation(), Some(2));
        let follow = consumer.current_work().cloned().unwrap();
        assert_eq!(follow.generation(), 2);
        assert_eq!(follow.kind(), WorkKind::Subscribe);
        // Losing the connection entirely also discards the result.
        let settled = settle(&mut consumer, &read, None, Ok(Some(snapshot)));
        assert!(matches!(settled, Settled::Stale));
        assert_eq!(consumer.generation(), None);
        assert!(consumer.current().is_none(), "stale result adopted");
    }

    #[test]
    fn replies_beyond_the_queue_are_shed_and_counted() {
        let binding = Binding {
            instance: "host".into(),
            profile: "default".into(),
        };
        let shared = status::shared();
        let mut consumer = Consumer::for_shell(binding).unwrap();
        let mut lane = Lane::default();
        let command = |sequence: usize| {
            BoundedIncomingEvent::Command(IncomingCommand {
                generation: 1,
                from: "peer".into(),
                command: "portald.status".into(),
                id: Some(sequence.to_string()),
                args: json!({}),
                body: String::new(),
                headers: BTreeMap::new(),
            })
        };
        let total = REPLY_QUEUE_LIMIT + 6;
        for sequence in 0..total {
            handle_event(&shared, &mut consumer, &mut lane, command(sequence)).unwrap();
        }
        assert_eq!(lane.replies.len(), REPLY_QUEUE_LIMIT);
        assert_eq!(status::report(&shared)["portal"]["replies_dropped"], 6);
        // The queue keeps arrival order, so the oldest requests are the ones answered.
        assert_eq!(
            lane.replies.front().unwrap().command.id.as_deref(),
            Some("0")
        );
    }
}
