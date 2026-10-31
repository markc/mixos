// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The ABP feed follows settingsd's contract 1.0.0. It subscribes to the
//! `appearance` and `window` keys topics; a delivery on either, stamped by
//! the broker as settingsd's or not, only asks for one authenticated read.
//! A read is `settings.get {view: values}` for the resolved mode and the
//! caption side, then `settings.palette` for that mode, fenced on one record
//! identity ([`Projection::from_answers`]). The feed handles reconnect
//! generations, bounded retries and hint pacing itself.
use crate::appearance::{self, Projection};
use crate::status::{self, Shared};
use bus::PortReply;
use bus::native_client::{
    BoundedIncomingEvent, BoundedIncomingReceiver, ConnState, IncomingCommand, SupervisedClient,
};
use serde_json::{Value, json};
use settings::native;
use settings::{Binding, Diagnostic, Revision};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub const COMMANDS: &[&str] = &["portald.status"];
pub const SOURCE: &str = "settingsd";
/// The first read of a projection: the resolved mode and the caption side.
pub const FETCH_COMMAND: &str = "settings.get";
/// The second: the contrast and the accent of the resolved mode.
pub const PALETTE_COMMAND: &str = "settings.palette";
/// The keys topics whose deliveries are hints: the resolved mode and the
/// accent move `appearance` (a `state.*` change included), the caption side
/// moves `window`.
pub const HINT_PREFIXES: [&str; 2] = ["appearance", "window"];
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(1);
const DEREGISTER_TIMEOUT: Duration = Duration::from_secs(3);
/// The first retry after a failed subscribe or read, doubling to [`RETRY_MAX`].
const RETRY_FIRST: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(30);

pub fn manifest() -> Vec<bus::CommandDescriptor> {
    manifest_for_screencast(crate::screencast::consent::installed().is_some())
}

fn manifest_for_screencast(screencast: bool) -> Vec<bus::CommandDescriptor> {
    std::iter::once(bus::CommandDescriptor::new(
        "HELP",
        &[],
        "List served commands",
        true,
    ))
    .chain(COMMANDS.iter().map(|command_name| {
        bus::CommandDescriptor::new(command_name, &[], "Settings portal status", true)
    }))
    .chain(
        crate::screencast::consent::COMMANDS
            .iter()
            .filter(|_| screencast)
            .map(|(command_name, description, read_only)| {
                bus::CommandDescriptor::new(command_name, &[], description, *read_only)
            }),
    )
    .collect()
}

/// The hint topics of `profile`.
pub fn hint_topics(profile: &str) -> Vec<String> {
    HINT_PREFIXES
        .iter()
        .map(|prefix| settings::topics::keys_topic(profile, prefix))
        .collect()
}

fn fault(command: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("read_failed", command, message)
}

/// One bounded call to settingsd, answered `current`.
async fn call(client: &SupervisedClient, command: &str, body: Value) -> Result<Value, Diagnostic> {
    let reply = client
        .call_typed(SOURCE, command, body)
        .await
        .map_err(|error| fault(command, error.to_string()))?;
    let value = match reply {
        PortReply::Ok { rc: 0, value } => value,
        PortReply::Ok { rc, value } => {
            return Err(fault(command, format!("{command} rc {rc}: {value}")));
        }
        PortReply::AppError { rc, message } => {
            return Err(fault(command, format!("{command} rc {rc}: {message}")));
        }
    };
    if serde_json::to_vec(&value)
        .map_err(|e| fault(command, e.to_string()))?
        .len()
        > settings::MAX_SNAPSHOT_BYTES + 64 * 1024
    {
        return Err(fault(command, "Response exceeds envelope budget"));
    }
    Ok(value)
}

/// One authenticated read: the values, then the palette of the mode they
/// resolve, at one record identity.
async fn fetch(client: &SupervisedClient, binding: &Binding) -> Result<Projection, Diagnostic> {
    let values = call(
        client,
        FETCH_COMMAND,
        json!({"binding": binding, "view": "values", "keys": appearance::VALUE_KEYS}),
    )
    .await?;
    let mode = values["resolved"]["mode"]
        .as_str()
        .ok_or_else(|| fault(FETCH_COMMAND, "Missing resolved mode"))?
        .to_owned();
    let palette = call(
        client,
        PALETTE_COMMAND,
        json!({"binding": binding, "mode": mode}),
    )
    .await?;
    Projection::from_answers(binding, &values, &palette)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkKind {
    Subscribe,
    Read,
}

/// One piece of work on one connection generation. At most one is current;
/// a superseded completion is harmless.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Work {
    generation: u64,
    ticket: u64,
    kind: WorkKind,
}

impl Work {
    pub fn kind(&self) -> WorkKind {
        self.kind
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// What the feed knows of its connection and of settingsd: the generation,
/// the work in flight, the retry after a failure and the last identity
/// forwarded (older is never forwarded within one incarnation).
pub struct Follow {
    binding: Binding,
    generation: Option<u64>,
    subscribed: bool,
    work: Option<Work>,
    next_ticket: u64,
    /// A read is wanted once the work in flight completes.
    wanted: bool,
    failures: u32,
    /// The work to repeat after [`Self::retry_delay`].
    retry: Option<WorkKind>,
    held: Option<(String, Revision)>,
    fault: Option<Diagnostic>,
}

impl Follow {
    pub fn new(binding: Binding) -> Self {
        Self {
            binding,
            generation: None,
            subscribed: false,
            work: None,
            next_ticket: 0,
            wanted: false,
            failures: 0,
            retry: None,
            held: None,
            fault: None,
        }
    }

    pub fn binding(&self) -> &Binding {
        &self.binding
    }

    pub fn generation(&self) -> Option<u64> {
        self.generation
    }

    pub fn current_work(&self) -> Option<&Work> {
        self.work.as_ref()
    }

    pub fn fault(&self) -> Option<&Diagnostic> {
        self.fault.as_ref()
    }

    fn issue(&mut self, kind: WorkKind) -> Option<Work> {
        let generation = self.generation?;
        self.next_ticket += 1;
        self.retry = None;
        let work = Work {
            generation,
            ticket: self.next_ticket,
            kind,
        };
        self.work = Some(work.clone());
        Some(work)
    }

    /// A connection at `generation`: subscribe on it, then read. The same
    /// generation again changes nothing.
    pub fn connected(&mut self, generation: u64) -> Option<Work> {
        if self.generation == Some(generation) {
            return None;
        }
        self.generation = Some(generation);
        self.subscribed = false;
        self.wanted = false;
        self.failures = 0;
        self.issue(WorkKind::Subscribe)
    }

    pub fn disconnected(&mut self) {
        self.generation = None;
        self.subscribed = false;
        self.work = None;
        self.wanted = false;
        self.retry = None;
    }

    /// Something may have changed: read now, or once the work in flight
    /// completes. A read in flight is never discarded.
    pub fn refresh(&mut self) {
        if self.generation.is_none() {
            return;
        }
        if self.work.is_some() {
            self.wanted = true;
            return;
        }
        let kind = if self.subscribed {
            WorkKind::Read
        } else {
            WorkKind::Subscribe
        };
        self.issue(kind);
    }

    /// After a failure, how long to wait before repeating the work.
    pub fn retry_delay(&self) -> Option<Duration> {
        (self.work.is_none() && self.retry.is_some()).then(|| {
            RETRY_FIRST
                .saturating_mul(1 << self.failures.saturating_sub(1).min(16))
                .min(RETRY_MAX)
        })
    }

    /// The retry is due: repeat the failed work.
    pub fn retry(&mut self) {
        if self.work.is_none()
            && let Some(kind) = self.retry
        {
            self.issue(kind);
        }
    }

    /// `work` finished with `result`. Returns the projection to forward: a
    /// read's, unless it is older than one already forwarded in the same
    /// incarnation.
    pub fn complete(
        &mut self,
        work: &Work,
        result: Result<Option<Projection>, Diagnostic>,
    ) -> Option<Projection> {
        if self.work.as_ref() != Some(work) {
            return None;
        }
        self.work = None;
        let wanted = std::mem::take(&mut self.wanted);
        match result {
            Err(error) => {
                self.failures = self.failures.saturating_add(1);
                self.fault = Some(error);
                // A hint meanwhile folds into the retry: a failing authority
                // is not asked again faster than the backoff.
                self.retry = Some(work.kind);
                None
            }
            Ok(projection) => {
                self.failures = 0;
                self.fault = None;
                match work.kind {
                    WorkKind::Subscribe => {
                        self.subscribed = true;
                        self.issue(WorkKind::Read);
                        None
                    }
                    WorkKind::Read => {
                        if wanted {
                            self.issue(WorkKind::Read);
                        }
                        let projection = projection?;
                        let (incarnation, revision) = projection.identity();
                        if let Some((held, at)) = &self.held
                            && held == incarnation
                            && revision < *at
                        {
                            return None;
                        }
                        self.held = Some((incarnation.to_owned(), revision));
                        Some(projection)
                    }
                }
            }
        }
    }
}

async fn execute(
    client: Arc<SupervisedClient>,
    work: Work,
    binding: Binding,
    fetch_timeout: Duration,
) -> Result<Option<Projection>, Diagnostic> {
    if native::live_generation(&client) != Some(work.generation()) {
        return Err(fault(FETCH_COMMAND, "Connection changed before the work"));
    }
    let result = match work.kind() {
        WorkKind::Subscribe => tokio::time::timeout(fetch_timeout, async {
            for topic in hint_topics(&binding.profile) {
                client.subscribe_topic(&topic).await.map_err(|error| {
                    Diagnostic::new("subscribe_failed", &topic, error.to_string())
                })?;
            }
            Ok(None)
        })
        .await
        .map_err(|_| Diagnostic::new("read_timeout", "subscribe", "Subscribe timed out"))?,
        WorkKind::Read => tokio::time::timeout(fetch_timeout, fetch(&client, &binding))
            .await
            .map_err(|_| {
                Diagnostic::new("read_timeout", FETCH_COMMAND, "Appearance read timed out")
            })?
            .map(Some),
    };
    if native::live_generation(&client) != Some(work.generation()) {
        return Err(fault(FETCH_COMMAND, "Connection changed during the work"));
    }
    result
}

struct Job {
    work: Work,
    future: Pin<Box<dyn Future<Output = Result<Option<Projection>, Diagnostic>> + Send>>,
}

async fn retry_after(delay: Option<Duration>) {
    match delay {
        Some(delay) => tokio::time::sleep(delay).await,
        None => std::future::pending::<()>().await,
    }
}

/// The outcome of one completion.
enum Settled {
    /// The work belonged to a superseded connection. Nothing was adopted.
    Stale,
    /// Completed on the live connection: the projection to forward, if any.
    Completed(Option<Projection>),
}

/// Settles a completion against the connection sampled just before it. A result from
/// a superseded connection is never completed. The feed moves to the live
/// connection instead, and the stale result is discarded.
fn settle(
    follow: &mut Follow,
    work: &Work,
    live: Option<u64>,
    result: Result<Option<Projection>, Diagnostic>,
) -> Settled {
    if live != Some(work.generation()) {
        match live {
            Some(generation) => {
                follow.connected(generation);
            }
            None => follow.disconnected(),
        }
        return Settled::Stale;
    }
    Settled::Completed(follow.complete(work, result))
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

/// Scheduling for unauthenticated hints, kept apart from the feed's own work. A
/// hint never discards a read in flight. Hints that arrive during a read coalesce
/// into one `pending` flag, and at most one follow-up read starts per completion.
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
    fn tick(&mut self, follow: &mut Follow) {
        if !self.pending || follow.generation().is_none() || follow.current_work().is_some() {
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
        follow.refresh();
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

/// How a topic delivery is judged. Only a frame on one of our hint topics is
/// judged at all; one without the broker's `settingsd` owner stamp is refused
/// with a stable reason code, never silently dropped.
#[derive(Debug, PartialEq, Eq)]
enum Admission {
    Admitted,
    Refused(&'static str),
    Other,
}

/// The owner stamp noded writes on reserved-publisher topics. Read only from
/// the verified broker frame.
const BROKER_SERVICE_HEADER: &str = "broker_service";

fn admit(binding: &Binding, command: &IncomingCommand) -> Admission {
    let ours = command.topic().is_some_and(|topic| {
        HINT_PREFIXES
            .iter()
            .any(|prefix| topic == settings::topics::keys_topic(&binding.profile, prefix))
    });
    if !ours {
        return Admission::Other;
    }
    match command.header(BROKER_SERVICE_HEADER) {
        Some("settingsd") => Admission::Admitted,
        Some(_) => Admission::Refused(native::REFUSED_WRONG_OWNER),
        None => Admission::Refused(native::REFUSED_MISSING_OWNER),
    }
}

/// Classifies one event from the bounded lane. Synchronous by design: the completion
/// drain calls it between a read's result and its completion, so it must not await.
/// Hints update the feed and hint state at once. Replies are queued for
/// `send_replies`.
fn handle_event(
    status: &Shared,
    follow: &mut Follow,
    lane: &mut Lane,
    event: BoundedIncomingEvent,
) -> anyhow::Result<()> {
    match event {
        BoundedIncomingEvent::Overflow { dropped } => {
            // A dropped command may have been a settings delivery, and the lane cannot
            // say which. Treat the loss as a paced hint.
            lane_overflowed(status, &mut lane.overflows, dropped);
            lane.hints.hint();
            lane.hints.tick(follow);
        }
        BoundedIncomingEvent::Command(command) => {
            if command.is_topic_delivery() {
                match admit(follow.binding(), &command) {
                    // settingsd's own delivery: the record moved, or settingsd
                    // (re)connected and republished, even at an unchanged
                    // revision. Its body is not read; one read follows.
                    Admission::Admitted => follow.refresh(),
                    Admission::Refused(reason) => {
                        // A refused frame is a paced hint: it only asks for one
                        // authenticated read, never more than one per interval.
                        refuse_delivery(status, &mut lane.refusals, reason);
                        lane.hints.hint();
                        lane.hints.tick(follow);
                    }
                    Admission::Other => {}
                }
                return Ok(());
            }
            let (rc, value) = match command.command.as_str() {
                "HELP" => (0, json!(manifest())),
                "portald.status" => {
                    let mut report = status::report(status);
                    report["settings_generation"] = json!(follow.generation());
                    (0, report)
                }
                command_name if command_name.starts_with("portald.screencast.") => {
                    crate::screencast::consent::handle_command(&command)
                }
                other => (
                    10,
                    json!({
                        "error_code": "UNKNOWN_COMMAND",
                        "message": format!("unknown command {other}"),
                        "command": other,
                    }),
                ),
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
            .with_commands(manifest())
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
    projections: mpsc::Sender<Projection>,
    status: Shared,
) -> anyhow::Result<()> {
    let (client, incoming) = connect(&noded_url)?;
    // The ScreenCast consent prompt posts its questions through this client.
    crate::screencast::consent::set_client(client.clone());
    run(binding, client, incoming, projections, status).await
}

/// The feed loop over an already connected client. It owns the client: it
/// deregisters and closes it before returning.
pub async fn run<I: CommandSource>(
    binding: Binding,
    client: Arc<SupervisedClient>,
    incoming: I,
    projections: mpsc::Sender<Projection>,
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
    projections: mpsc::Sender<Projection>,
    status: Shared,
    fetch_timeout: Duration,
) -> anyhow::Result<()> {
    if let Err(error) = binding.validate() {
        client.close().await;
        anyhow::bail!(error.message);
    }
    let mut follow = Follow::new(binding);
    let mut state = client.subscribe_state();
    let mut job: Option<Job> = None;
    let mut lane = Lane::default();
    let result = loop {
        // Replies classified by a drain go out here, before any state is sampled.
        send_replies(&client, &mut lane.replies).await;
        // Sample INITIAL state and every generation before waiting for edges.
        // Follow::connected deduplicates already processed generations.
        let now = *state.borrow_and_update();
        if now == ConnState::Fatal {
            break Err(anyhow::anyhow!("portald registration rejected"));
        }
        match native::live_generation(&client) {
            Some(generation) => {
                follow.connected(generation);
            }
            None if follow.generation().is_some() => follow.disconnected(),
            None => {}
        }
        // Replacing this future cancels superseded work without spawning another
        // worker. Status requests and lifecycle events stay responsive during reads.
        if job.as_ref().map(|job| &job.work) != follow.current_work() {
            job = follow.current_work().cloned().map(|work| {
                if work.kind() == WorkKind::Read {
                    status::update(&status, |s| s.authority_reads += 1);
                }
                Job {
                    future: Box::pin(execute(
                        client.clone(),
                        work.clone(),
                        follow.binding().clone(),
                        fetch_timeout,
                    )),
                    work,
                }
            });
        }
        let retry = follow.retry_delay();
        // Unbiased on purpose. Fairness is not what orders a hint before a
        // completion: the completion arm drains the queue itself before it
        // completes. A biased event-first select would starve completions under
        // a sustained event stream.
        tokio::select! {
            event = incoming.next_event() => {
                match event {
                    None => break Ok(()),
                    Some(event) => handle_event(&status, &mut follow, &mut lane, event)?,
                }
            }
            _ = projections.closed() => break Ok(()),
            changed = state.changed() => {
                if changed.is_err() { break Ok(()); }
            }
            _ = retry_after(retry) => { follow.retry(); }
            result = async { job.as_mut().expect("enabled job").future.as_mut().await }, if job.is_some() => {
                let completed = job.take().expect("completed job");
                // Queued events count before this completion schedules anything. A
                // hint already in the lane is part of the same burst, so it must not
                // cost a second read. The drain is bounded, and it never awaits, so a
                // sustained stream cannot hold the completion forever.
                for _ in 0..DRAIN_LIMIT {
                    let Some(event) = incoming.try_next_event() else { break };
                    handle_event(&status, &mut follow, &mut lane, event)?;
                }
                // No await since the drain: the connection is sampled immediately before
                // the completion it fences.
                let live = native::live_generation(&client);
                match settle(&mut follow, &completed.work, live, result) {
                    Settled::Stale => {}
                    Settled::Completed(projection) => {
                        if let Some(projection) = projection
                            && projections.send(projection).await.is_err()
                        {
                            break Ok(());
                        }
                        if let Some(error) = follow.fault() {
                            status::update(&status, |s| s.settings_fault = Some(error.message.clone()));
                        }
                        lane.hints.tick(&mut follow);
                    }
                }
            }
            _ = hint_due(lane.hints.due), if lane.hints.due.is_some() => {
                lane.hints.due = None;
                lane.hints.tick(&mut follow);
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

    fn binding() -> Binding {
        Binding {
            instance: "host".into(),
            profile: "default".into(),
        }
    }

    fn projection(revision: u64) -> Projection {
        Projection {
            schema: appearance::PROJECTION_SCHEMA,
            binding: binding(),
            incarnation: "a".into(),
            revision: Revision(revision),
            mode: "dark".into(),
            contrast: "normal".into(),
            caption_side: "right".into(),
            accent: [0.1, 0.2, 0.3],
        }
    }

    /// A follow on generation 1, subscribed, with its first read in flight.
    fn reading() -> (Follow, Work) {
        let mut follow = Follow::new(binding());
        let subscribe = follow.connected(1).unwrap();
        assert_eq!(subscribe.kind(), WorkKind::Subscribe);
        assert!(follow.complete(&subscribe, Ok(None)).is_none());
        let read = follow.current_work().cloned().unwrap();
        assert_eq!(read.kind(), WorkKind::Read);
        (follow, read)
    }

    #[test]
    fn help_only_lists_commands_enabled_for_this_instance() {
        for enabled in [false, true] {
            let commands = manifest_for_screencast(enabled);
            assert_eq!(commands[0].name, "HELP");
            assert!(commands[0].read_only);
            for (name, _, _) in crate::screencast::consent::COMMANDS {
                assert_eq!(
                    commands.iter().any(|command| command.name == *name),
                    enabled
                );
            }
            if !enabled {
                assert_eq!(commands.len(), 2);
                assert_eq!(commands[1].name, "portald.status");
            }
        }
    }

    /// The Settings side never writes. The only mutating commands are the
    /// ScreenCast consent ones (answer, grant, revoke), which change consent
    /// state and nothing in settings.
    #[test]
    fn the_manifest_is_read_only_but_for_screencast_consent() {
        let manifest = manifest_for_screencast(true);
        let writers: Vec<_> = manifest
            .iter()
            .filter(|command_name| !command_name.read_only)
            .map(|command_name| command_name.name.as_str())
            .collect();
        assert_eq!(
            writers,
            [
                "portald.screencast.answer",
                "portald.screencast.grant",
                "portald.screencast.revoke"
            ]
        );
        assert!(
            manifest
                .iter()
                .any(|command_name| command_name.name == "portald.status")
        );
    }

    #[test]
    fn it_hints_on_the_appearance_and_window_keys_topics() {
        assert_eq!(
            hint_topics("default"),
            [
                "settingsd.desktop.keys.default.appearance",
                "settingsd.desktop.keys.default.window",
            ]
        );
        let delivery = |topic: &str, stamp: Option<&str>| {
            let mut headers = BTreeMap::from([("topic".to_owned(), topic.to_owned())]);
            if let Some(stamp) = stamp {
                headers.insert(BROKER_SERVICE_HEADER.into(), stamp.into());
            }
            IncomingCommand {
                generation: 1,
                from: "noded".into(),
                command: topic.into(),
                id: None,
                args: json!({}),
                body: "{}".into(),
                headers,
            }
        };
        let ours = "settingsd.desktop.keys.default.window";
        assert_eq!(
            admit(&binding(), &delivery(ours, Some("settingsd"))),
            Admission::Admitted
        );
        assert_eq!(
            admit(&binding(), &delivery(ours, None)),
            Admission::Refused(native::REFUSED_MISSING_OWNER)
        );
        assert_eq!(
            admit(&binding(), &delivery(ours, Some("peer"))),
            Admission::Refused(native::REFUSED_WRONG_OWNER)
        );
        for other in [
            "settingsd.desktop.keys.default.displays",
            "settingsd.desktop.keys.other.appearance",
            "settingsd.desktop.compact.default",
        ] {
            assert_eq!(
                admit(&binding(), &delivery(other, Some("settingsd"))),
                Admission::Other
            );
        }
    }

    #[test]
    fn hints_during_a_read_merge_into_one_follow_up() {
        let (mut follow, read) = reading();
        let mut hints = Hints::default();
        // A burst during the in-flight read discards nothing and starts nothing.
        for _ in 0..3 {
            hints.hint();
            hints.tick(&mut follow);
        }
        assert!(follow.current_work() == Some(&read));
        // The read completes. The first tick after it starts exactly one follow-up.
        assert!(follow.complete(&read, Ok(Some(projection(2)))).is_some());
        hints.tick(&mut follow);
        let follow_up = follow.current_work().cloned().unwrap();
        assert_eq!(follow_up.kind(), WorkKind::Read);
        assert!(!hints.pending);
        // Hints during the follow-up wait for it: no second read is in flight.
        hints.hint();
        hints.tick(&mut follow);
        assert!(follow.current_work() == Some(&follow_up));
        assert!(hints.pending);
    }

    #[test]
    fn a_refresh_during_a_read_costs_one_read_after_it() {
        let (mut follow, read) = reading();
        follow.refresh();
        follow.refresh();
        assert!(follow.current_work() == Some(&read), "never discarded");
        assert!(follow.complete(&read, Ok(Some(projection(1)))).is_some());
        let again = follow.current_work().cloned().unwrap();
        assert_ne!(again, read);
        assert!(follow.complete(&again, Ok(Some(projection(1)))).is_some());
        assert!(follow.current_work().is_none());
    }

    #[test]
    fn an_older_revision_is_never_forwarded_a_new_incarnation_is() {
        let (mut follow, read) = reading();
        assert!(follow.complete(&read, Ok(Some(projection(5)))).is_some());
        follow.refresh();
        let read = follow.current_work().cloned().unwrap();
        assert!(follow.complete(&read, Ok(Some(projection(4)))).is_none());
        follow.refresh();
        let read = follow.current_work().cloned().unwrap();
        let mut restarted = projection(1);
        restarted.incarnation = "b".into();
        assert!(follow.complete(&read, Ok(Some(restarted))).is_some());
    }

    #[test]
    fn a_failure_is_retried_after_a_doubling_delay() {
        let (mut follow, read) = reading();
        assert!(follow.retry_delay().is_none());
        let failed = || Err(Diagnostic::new("read_failed", "x", "down"));
        assert!(follow.complete(&read, failed()).is_none());
        assert_eq!(follow.retry_delay(), Some(RETRY_FIRST));
        assert_eq!(follow.fault().unwrap().message, "down");
        follow.retry();
        let again = follow.current_work().cloned().unwrap();
        assert_eq!(again.kind(), WorkKind::Read);
        assert!(follow.complete(&again, failed()).is_none());
        assert_eq!(follow.retry_delay(), Some(RETRY_FIRST * 2));
        follow.retry();
        let again = follow.current_work().cloned().unwrap();
        assert!(follow.complete(&again, Ok(Some(projection(1)))).is_some());
        assert!(follow.retry_delay().is_none() && follow.fault().is_none());
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
        let shared = status::shared();
        let (mut follow, read) = reading();
        let mut lane = Lane::default();
        // A lane overflow during the read is a hint: the read stays in flight and
        // the follow-up is only wanted.
        for _ in 0..50 {
            handle_event(
                &shared,
                &mut follow,
                &mut lane,
                BoundedIncomingEvent::Overflow { dropped: 3 },
            )
            .unwrap();
        }
        assert!(follow.current_work() == Some(&read));
        assert!(lane.hints.pending);
        // The read completes with its own result. It is forwarded, not replaced.
        let settled = settle(&mut follow, &read, Some(1), Ok(Some(projection(1))));
        assert!(matches!(settled, Settled::Completed(Some(p)) if p.revision == Revision(1)));
        assert_eq!(status::report(&shared)["portal"]["lane_dropped"], 150);
    }

    #[test]
    fn a_read_from_a_superseded_connection_is_never_adopted() {
        let (mut follow, read) = reading();
        // The connection moved to generation 2 before the completion. The feed
        // still holds generation 1, so without the fence it would accept this result.
        let settled = settle(&mut follow, &read, Some(2), Ok(Some(projection(2))));
        assert!(matches!(settled, Settled::Stale));
        assert_eq!(follow.generation(), Some(2));
        let next = follow.current_work().cloned().unwrap();
        assert_eq!(next.generation(), 2);
        assert_eq!(next.kind(), WorkKind::Subscribe);
        // Losing the connection entirely also discards the result.
        let settled = settle(&mut follow, &read, None, Ok(Some(projection(2))));
        assert!(matches!(settled, Settled::Stale));
        assert_eq!(follow.generation(), None);
        assert!(follow.current_work().is_none());
    }

    #[test]
    fn replies_beyond_the_queue_are_shed_and_counted() {
        let shared = status::shared();
        let mut follow = Follow::new(binding());
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
            handle_event(&shared, &mut follow, &mut lane, command(sequence)).unwrap();
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
