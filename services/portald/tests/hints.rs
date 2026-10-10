// SPDX-License-Identifier: MIT OR Apache-2.0
//! Feed-level hint scheduling against the real test broker.
//!
//! A scripted settingsd registers under the settingsd name, so the broker stamps
//! its publications exactly as it stamps the real authority's. It answers
//! `settings.appearance.get` from a revision the test controls. Hints are unstamped
//! frames, which a current noded never lets a non-owner publish on the settings
//! topic, so they enter the feed through a bounded lane. That lane's producer is the
//! production sender (`bounded_incoming_injector`), so an injected frame overflows
//! exactly as a broker frame does, with the same 64-slot capacity as the feed's own.
//!
//! Timing does not decide any count. Every authority read waits for a permit that
//! only the test adds, and each hint is counted through the feed's status before
//! the read it concerns is released. The overflow tests run on a single thread, so
//! a burst is synchronous: the feed cannot drain part of it, and the number dropped
//! is exact. The only real-time waits are the spacing bound, which a stall can only
//! relax, and the quiet windows that check for reads which must not happen.
use bus::native_client::{
    BoundedIncomingEvent, BoundedIncomingInjector, BoundedIncomingReceiver, IncomingCommand,
    SupervisedClient, bounded_incoming_injector,
};
use portald::service::{self, CommandSource};
use portald::status::{self, Shared};
use serde_json::json;
use settings::appearance::AppearanceProjection;
use settings::{Binding, Desktop, Revision, Snapshot};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;

/// The spacing the feed promises between hint-driven reads.
const INTERVAL_MS: u64 = 250;
/// How long to watch for a read that must not happen.
const QUIET: Duration = Duration::from_millis(600);
const WAIT: Duration = Duration::from_secs(10);
/// The first hint-driven read must start within the interval plus this slack. It is
/// a generous ceiling for a loaded host, and a stall beyond it is a failure.
const PROMPT_SLACK: Duration = Duration::from_secs(1);
/// For the tests that hold a read deliberately across a burst of events. A held read
/// must not time out while the test drives the burst. Every other test uses the
/// production fetch timeout.
const HELD_READ_TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(5);
/// The production lane's capacity: `service::connect` uses `bounded_incoming(64)`.
const LANE: u64 = 64;
/// One synchronous burst, well past the lane, so every burst overflows it.
const BURST: u64 = 200;
/// The most injected frames that fit in the lane at once, for paced floods.
const PACE: u64 = 32;

fn binding() -> Binding {
    Binding {
        instance: "host".into(),
        profile: "default".into(),
    }
}

fn snapshot(revision: u64) -> Snapshot {
    let desktop = Desktop::default();
    let effective = settings::resolve(&desktop).unwrap();
    Snapshot {
        schema: settings::SCHEMA,
        binding: binding(),
        incarnation: "a".into(),
        revision: Revision(revision),
        design_revision: Revision(1),
        source_digest: "source".into(),
        desktop,
        effective,
    }
}

/// The most hint-driven read starts that can fit in `elapsed`. Consecutive starts
/// are at least one interval apart, and two more starts of slack cover the first
/// start in the span and a consumer-driven read that began just before it.
fn bound(elapsed: Duration) -> u64 {
    elapsed.as_millis() as u64 / INTERVAL_MS + 2
}

fn free_port() -> anyhow::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

/// The scripted authority's state. A read captures the revision when it arrives,
/// then waits for a permit, then answers with the captured snapshot.
struct Authority {
    state: Mutex<Snapshot>,
    /// Reads that have arrived, whether or not they have been answered.
    reads: AtomicU64,
    /// One permit answers one read. The test adds permits to release reads.
    gate: Semaphore,
    /// Each captured read, as `read revN @Tms`, for failure timelines.
    captured: Mutex<Vec<String>>,
    /// When each read arrived, in arrival order.
    arrivals: Mutex<Vec<Instant>>,
    started: Instant,
}

async fn serve(
    client: Arc<SupervisedClient>,
    mut incoming: BoundedIncomingReceiver,
    authority: Arc<Authority>,
) {
    while let Some(event) = incoming.recv().await {
        let BoundedIncomingEvent::Command(command) = event else {
            continue;
        };
        if command.is_topic_delivery() || command.command != service::FETCH_VERB {
            continue;
        }
        let snapshot = authority.state.lock().unwrap().clone();
        authority.arrivals.lock().unwrap().push(Instant::now());
        authority.captured.lock().unwrap().push(format!(
            "read rev{} @{}ms",
            snapshot.revision.0,
            authority.started.elapsed().as_millis()
        ));
        authority.reads.fetch_add(1, Ordering::SeqCst);
        let Ok(permit) = authority.gate.acquire().await else {
            break;
        };
        permit.forget();
        let appearance = AppearanceProjection::from_snapshot(&snapshot, [0.1, 0.2, 0.3]).unwrap();
        let body = json!({
            "status": "current",
            "appearance": appearance,
            "snapshot": snapshot,
        })
        .to_string();
        let _ = client.respond(&command, 0, &body).await;
    }
}

/// An unstamped topic frame, as an old noded delivers one.
fn hint() -> IncomingCommand {
    let topic = settings::topic("default");
    IncomingCommand {
        generation: 0,
        from: "noded".into(),
        command: topic.clone(),
        id: None,
        args: json!({}),
        body: "{}".into(),
        headers: BTreeMap::from([("topic".into(), topic)]),
    }
}

/// The real broker stream, with the injected bounded lane beside it.
struct Injected {
    real: BoundedIncomingReceiver,
    injected: BoundedIncomingReceiver,
}

impl CommandSource for Injected {
    async fn next_event(&mut self) -> Option<BoundedIncomingEvent> {
        tokio::select! {
            event = self.injected.recv() => event,
            event = self.real.recv() => event,
        }
    }

    fn try_next_event(&mut self) -> Option<BoundedIncomingEvent> {
        self.injected.try_recv().or_else(|| self.real.try_recv())
    }
}

struct Harness {
    broker: test_broker::Broker,
    authority: Arc<Authority>,
    publisher: Arc<SupervisedClient>,
    injector: BoundedIncomingInjector,
    projections: mpsc::Receiver<AppearanceProjection>,
    /// Every projection the feed forwarded, as `projected revN @Tms`.
    log: Vec<String>,
    shared: Shared,
    serve: JoinHandle<()>,
    feed: JoinHandle<anyhow::Result<()>>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.serve.abort();
        self.feed.abort();
    }
}

async fn wait_until(what: &str, mut done: impl FnMut() -> bool) -> anyhow::Result<()> {
    tokio::time::timeout(WAIT, async {
        while !done() {
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for {what}"))
}

impl Harness {
    /// Starts the authority at revision 1 and portald on the same broker. The
    /// authority holds every read until the test allows it.
    /// `fetch_timeout` is the feed's own timeout for an authority read. A test that
    /// holds a read across a burst passes `HELD_READ_TIMEOUT`; the others pass the
    /// production `service::FETCH_TIMEOUT`.
    async fn start(fetch_timeout: Duration) -> anyhow::Result<Self> {
        Self::start_on(None, fetch_timeout).await
    }

    /// `start`, on a fixed TCP port when one is given, so `bounce` returns to it.
    async fn start_on(port: Option<u16>, fetch_timeout: Duration) -> anyhow::Result<Self> {
        let broker = match port {
            Some(port) => test_broker::Broker::with_tcp_port(port),
            None => test_broker::Broker::start(),
        };
        let url = broker.url.clone();
        let publisher = Arc::new(
            SupervisedClient::connect_options("settingsd", &url)
                .bounded_incoming(64)
                .connect()
                .await?,
        );
        let incoming = publisher
            .incoming_bounded()
            .ok_or_else(|| anyhow::anyhow!("authority incoming taken"))?;
        let authority = Arc::new(Authority {
            state: Mutex::new(snapshot(1)),
            reads: AtomicU64::new(0),
            gate: Semaphore::new(0),
            captured: Mutex::new(Vec::new()),
            arrivals: Mutex::new(Vec::new()),
            started: Instant::now(),
        });
        let serve = tokio::spawn(serve(publisher.clone(), incoming, authority.clone()));
        let (client, real) = service::connect(&url)?;
        let (injector, injected) = bounded_incoming_injector(LANE as usize);
        let (projection_tx, projections) = mpsc::channel(64);
        let shared = status::shared();
        let feed = tokio::spawn(service::run_with_fetch_timeout(
            binding(),
            client,
            Injected { real, injected },
            projection_tx,
            shared.clone(),
            fetch_timeout,
        ));
        Ok(Self {
            broker,
            authority,
            publisher,
            injector,
            projections,
            log: Vec::new(),
            shared,
            serve,
            feed,
        })
    }

    /// The authority's captured reads and the forwarded projections, in order.
    fn timeline(&self) -> String {
        format!(
            "authority {:?}; projections {:?}",
            self.authority.captured.lock().unwrap(),
            self.log
        )
    }

    fn record(&mut self, projection: &AppearanceProjection) -> u64 {
        self.log.push(format!(
            "projected rev{} @{}ms",
            projection.revision.0,
            self.authority.started.elapsed().as_millis()
        ));
        projection.revision.0
    }

    /// Reads that have arrived at the authority.
    fn reads(&self) -> u64 {
        self.authority.reads.load(Ordering::SeqCst)
    }

    fn portal(&self, field: &str) -> u64 {
        status::report(&self.shared)["portal"][field]
            .as_u64()
            .unwrap_or(0)
    }

    /// Hints the feed has refused, as counted in its status.
    fn refusals(&self) -> u64 {
        self.portal("delivery_refusals")
    }

    /// Refused-delivery warning lines the feed has logged.
    fn reports(&self) -> u64 {
        self.portal("refusal_reports")
    }

    /// Lane commands dropped by overflow, counted exactly.
    fn lane_dropped(&self) -> u64 {
        self.portal("lane_dropped")
    }

    /// Overflow warning lines the feed has logged.
    fn overflow_reports(&self) -> u64 {
        self.portal("overflow_reports")
    }

    /// When the `index`th read (0 is the bootstrap read) arrived at the authority.
    fn arrival(&self, index: usize) -> Option<Instant> {
        self.authority.arrivals.lock().unwrap().get(index).copied()
    }

    /// Hint-driven and consumer-driven reads the feed has started.
    fn reported_reads(&self) -> u64 {
        self.portal("authority_reads")
    }

    fn set_revision(&self, revision: u64) {
        *self.authority.state.lock().unwrap() = snapshot(revision);
    }

    /// Releases `reads` held reads, each answered with the snapshot it captured.
    fn allow(&self, reads: usize) {
        self.authority.gate.add_permits(reads);
    }

    /// One hint, through the production lane. The lane may drop it when full, so
    /// tests that need every hint counted keep fewer than `LANE` in flight.
    fn hint(&self) {
        assert!(self.injector.try_send(hint()), "feed is running");
    }

    /// A synchronous burst of `BURST` hints. On one thread nothing else runs until it
    /// returns, so the lane keeps exactly `LANE` and drops the rest. Waits until the
    /// feed has counted that loss, and checks the count is exact.
    async fn overflow(&self) -> anyhow::Result<()> {
        let before = self.lane_dropped();
        for _ in 0..BURST {
            self.hint();
        }
        let expected = before + BURST - LANE;
        wait_until("the lane overflow to be counted", || {
            self.lane_dropped() >= expected
        })
        .await
        .map_err(|error| anyhow::anyhow!("{error}; {}", self.timeline()))?;
        anyhow::ensure!(
            self.lane_dropped() == expected,
            "lane dropped {} for a burst of {BURST}, expected {}; {}",
            self.lane_dropped() - before,
            BURST - LANE,
            self.timeline()
        );
        Ok(())
    }

    /// `count` hints, sent in chunks that always fit the lane. Each chunk is sent only
    /// after the feed has counted the previous one, so nothing is dropped.
    async fn hint_paced(&self, count: u64) -> anyhow::Result<()> {
        let refused = self.refusals();
        let mut sent = 0;
        while sent < count {
            let chunk = (count - sent).min(PACE);
            for _ in 0..chunk {
                self.hint();
            }
            sent += chunk;
            self.wait_refusals(refused + sent).await?;
        }
        Ok(())
    }

    async fn wait_reads(&self, at_least: u64) -> anyhow::Result<()> {
        wait_until(&format!("{at_least} authority reads"), || {
            self.reads() >= at_least
        })
        .await
        .map_err(|error| anyhow::anyhow!("{error}; {}", self.timeline()))
    }

    async fn wait_refusals(&self, at_least: u64) -> anyhow::Result<()> {
        wait_until(&format!("{at_least} processed hints"), || {
            self.refusals() >= at_least
        })
        .await
        .map_err(|error| anyhow::anyhow!("{error}; {}", self.timeline()))
    }

    /// Publishes the authority's current state as a stamped settingsd frame.
    async fn publish_stamped(&self) -> anyhow::Result<()> {
        let topic = settings::topic("default");
        let body = serde_json::to_string(&*self.authority.state.lock().unwrap())?;
        let mut inner = bus::wire::BusMessage::new();
        inner.set("command", &topic);
        inner.body = body;
        let headers = BTreeMap::from([("name".into(), topic), ("retain".into(), "true".into())]);
        self.publisher
            .call_with_headers("noded", "topic.publish", &headers, &inner.to_wire())
            .await?;
        Ok(())
    }

    /// Projections already forwarded, in order. Never waits.
    fn drain_projections(&mut self) -> Vec<u64> {
        let mut seen = Vec::new();
        while let Ok(projection) = self.projections.try_recv() {
            seen.push(self.record(&projection));
        }
        seen
    }

    /// Waits until portald projects `revision`, skipping any earlier projection.
    async fn expect_revision(&mut self, revision: u64) -> anyhow::Result<()> {
        let outcome = tokio::time::timeout(WAIT, async {
            while let Some(projection) = self.projections.recv().await {
                if self.record(&projection) == revision {
                    return Ok(());
                }
            }
            anyhow::bail!("projection channel closed")
        })
        .await;
        match outcome {
            Ok(result) => result,
            Err(_) => anyhow::bail!(
                "no projection at revision {revision}; reads {}; portal {}; {}",
                self.reads(),
                status::report(&self.shared),
                self.timeline()
            ),
        }
    }
}

/// Status requests and unpaced hints, run beside a held read. Both stop when the
/// flood is stopped. The hints are not paced, so the lane overflows under them.
struct Flood {
    stop: Arc<std::sync::atomic::AtomicBool>,
    replies: Arc<AtomicU64>,
    status: JoinHandle<()>,
    hints: JoinHandle<()>,
}

impl Flood {
    fn start(harness: &Harness) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let replies = Arc::new(AtomicU64::new(0));
        let client = harness.publisher.clone();
        let status = tokio::spawn({
            let (stop, replies) = (stop.clone(), replies.clone());
            async move {
                while !stop.load(Ordering::SeqCst) {
                    let headers = BTreeMap::<String, String>::new();
                    if client
                        .call_with_headers("portald", "portald.status", &headers, "")
                        .await
                        .is_ok()
                    {
                        replies.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        });
        let injector = harness.injector.clone();
        let hint_task = tokio::spawn({
            let stop = stop.clone();
            async move {
                while !stop.load(Ordering::SeqCst) {
                    injector.try_send(hint());
                    tokio::task::yield_now().await;
                }
            }
        });
        Self {
            stop,
            replies,
            status,
            hints: hint_task,
        }
    }

    fn replies(&self) -> u64 {
        self.replies.load(Ordering::SeqCst)
    }

    async fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.status.await;
        let _ = self.hints.await;
    }
}

/// What a hint storm observed.
struct Storm {
    /// Reads the authority received during the storm.
    reads: u64,
    elapsed: Duration,
    /// Revisions projected by the time the storm's last read was released.
    projected: Vec<u64>,
}

/// One hint per read, for `rounds` reads, plus the final read the last hint leads
/// to. Each read is released only after the hint that follows it has been counted.
/// The seed hint starts the chain. The storm therefore expects exactly
/// `rounds + 1` reads, and each one is hint-driven except possibly the first.
async fn hint_storm(h: &mut Harness, base: u64, rounds: u64) -> anyhow::Result<Storm> {
    let refused = h.refusals();
    let started = Instant::now();
    let mut projected = Vec::new();
    h.hint();
    h.wait_refusals(refused + 1).await?;
    for round in 1..=rounds + 1 {
        h.wait_reads(base + round).await?;
        projected.extend(h.drain_projections());
        if round <= rounds {
            h.hint();
            h.wait_refusals(refused + round + 1).await?;
        }
        h.allow(1);
    }
    Ok(Storm {
        reads: h.reads() - base,
        elapsed: started.elapsed(),
        projected,
    })
}

/// A finite burst overlaps the in-flight read, and the authority moves to revision
/// 2 during it. Exactly one follow-up read converges, and then reads stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_finite_hint_burst_converges_with_exactly_one_follow_up_read() -> anyhow::Result<()> {
    let mut h = Harness::start(HELD_READ_TIMEOUT).await?;
    h.wait_reads(1).await?;
    h.allow(1);
    h.expect_revision(1).await?;
    let before = h.reads();
    anyhow::ensure!(before == 1, "bootstrap reads once, saw {before}");

    // A hint starts a read at once, since no interval has run yet. It is held.
    h.hint();
    h.wait_reads(before + 1).await?;
    h.set_revision(2);
    let counted = h.refusals();
    for _ in 0..4 {
        h.hint();
    }
    h.wait_refusals(counted + 4).await?;
    // The held read answers with its captured revision 1. The four hints are
    // already counted, so they must not cost a read of their own.
    h.allow(1);
    h.expect_revision(1).await?;
    h.wait_reads(before + 2).await?;
    h.allow(1);
    h.expect_revision(2).await?;

    tokio::time::sleep(QUIET).await;
    let total = h.reads();
    anyhow::ensure!(
        total == before + 2,
        "expected in-flight + one follow-up = {} reads, saw {total}; {}",
        before + 2,
        h.timeline()
    );
    anyhow::ensure!(
        h.reported_reads() == total,
        "status counts the same reads: {} vs {total}",
        h.reported_reads()
    );
    Ok(())
}

/// Hints arrive one per read while the first projection is still unconfirmed.
/// Revision 1 must be projected inside the storm, and reads stay within the
/// interval.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuous_hints_during_bootstrap_still_confirm_within_the_interval() -> anyhow::Result<()>
{
    let mut h = Harness::start(service::FETCH_TIMEOUT).await?;
    let storm = hint_storm(&mut h, 0, 6).await?;
    anyhow::ensure!(
        storm.projected.contains(&1),
        "no projection at revision 1 before the last read of the storm; projected {:?}; {}",
        storm.projected,
        h.timeline()
    );
    anyhow::ensure!(
        storm.reads == 7,
        "expected one read per hint plus the bootstrap read, 7; saw {}; {}",
        storm.reads,
        h.timeline()
    );
    anyhow::ensure!(
        storm.reads <= bound(storm.elapsed),
        "bootstrap reads {} exceed the interval bound {} for {:?}",
        storm.reads,
        bound(storm.elapsed),
        storm.elapsed
    );
    Ok(())
}

/// After activation, a hint per read stays within the interval, and a later
/// revision still converges with one more read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuous_hints_after_activation_stay_within_the_interval() -> anyhow::Result<()> {
    let mut h = Harness::start(service::FETCH_TIMEOUT).await?;
    h.wait_reads(1).await?;
    h.allow(1);
    h.expect_revision(1).await?;
    let before = h.reads();

    let storm = hint_storm(&mut h, before, 6).await?;
    anyhow::ensure!(
        storm.reads == 7,
        "expected 7 active reads for 7 hints; saw {}; {}",
        storm.reads,
        h.timeline()
    );
    anyhow::ensure!(
        storm.reads <= bound(storm.elapsed),
        "active reads {} exceed the interval bound {} for {:?}",
        storm.reads,
        bound(storm.elapsed),
        storm.elapsed
    );

    let after_storm = h.reads();
    h.set_revision(2);
    h.hint();
    h.wait_reads(after_storm + 1).await?;
    h.allow(1);
    h.expect_revision(2).await?;
    tokio::time::sleep(QUIET).await;
    anyhow::ensure!(
        h.reads() == after_storm + 1,
        "one hint after the storm must cost one read; saw {} from {after_storm}; {}",
        h.reads() - after_storm,
        h.timeline()
    );
    Ok(())
}

/// A stamped settingsd delivery takes the trusted path: it refreshes the
/// authority once and projects the new revision.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stamped_delivery_takes_the_trusted_path_with_one_read() -> anyhow::Result<()> {
    let mut h = Harness::start(service::FETCH_TIMEOUT).await?;
    h.wait_reads(1).await?;
    h.allow(1);
    h.expect_revision(1).await?;
    let before = h.reads();

    h.set_revision(2);
    h.publish_stamped().await?;
    h.wait_reads(before + 1).await?;
    h.allow(1);
    h.expect_revision(2).await?;
    tokio::time::sleep(QUIET).await;
    anyhow::ensure!(
        h.reads() == before + 1,
        "stamped delivery: expected one read, saw {} from {before}; {}",
        h.reads() - before,
        h.timeline()
    );
    Ok(())
}

/// The first hint after bootstrap starts its read promptly: no interval has run, so
/// only the scheduling could delay it. Then a held read runs while status requests
/// and unpaced hints flood the lane. The lane overflows under the flood, and the
/// read still completes and confirms, with reads within the interval.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sustained_event_stream_does_not_starve_the_read_completion() -> anyhow::Result<()> {
    let mut h = Harness::start(HELD_READ_TIMEOUT).await?;
    h.wait_reads(1).await?;
    h.allow(1);
    h.expect_revision(1).await?;
    let before = h.reads();
    let started = Instant::now();
    let reported = h.reported_reads();
    let refused = h.refusals();
    let dropped = h.lane_dropped();

    let sent = Instant::now();
    h.hint();
    h.wait_reads(before + 1).await?;
    let arrived = h
        .arrival(before as usize)
        .ok_or_else(|| anyhow::anyhow!("first hint read has no arrival time"))?;
    let latency = arrived.duration_since(sent);
    anyhow::ensure!(
        latency <= Duration::from_millis(INTERVAL_MS) + PROMPT_SLACK,
        "first hint read started {latency:?} after its hint; {}",
        h.timeline()
    );
    // Revision 2 is set only after the held read captured revision 1, so that read
    // answers revision 1 and the flood's hints cost one follow-up for revision 2.
    h.set_revision(2);

    let flood = Flood::start(&h);
    let flooded = async {
        wait_until("200 status replies during the held read", || {
            flood.replies() >= 200
        })
        .await?;
        wait_until("200 events processed during the held read", || {
            h.refusals() + h.lane_dropped() >= refused + dropped + 200
        })
        .await?;
        // The read completes while the flood still runs.
        h.allow(1);
        h.expect_revision(1).await?;
        h.wait_reads(before + 2).await?;
        h.allow(1);
        h.expect_revision(2).await?;
        anyhow::Ok(())
    }
    .await;
    flood.stop().await;
    flooded.map_err(|error| anyhow::anyhow!("{error}; {}", h.timeline()))?;

    let starts = h.reported_reads() - reported;
    let elapsed = started.elapsed();
    anyhow::ensure!(
        starts <= bound(elapsed),
        "reads under a sustained stream: {starts} starts in {elapsed:?}, bound {}",
        bound(elapsed)
    );
    Ok(())
}

/// The lane overflows during every held read of a live update. Each overflow is a
/// hint, so the feed never invalidates the read in flight: bootstrap confirms, the
/// live update lands, and reads stay within the interval.
///
/// The test runs on one thread. Each burst is synchronous, so the number dropped is
/// exact, and the feed cannot drain part of a burst before the next read is released.
#[tokio::test(flavor = "current_thread")]
async fn bootstrap_confirms_and_live_updates_land_while_every_held_read_overflows_the_lane()
-> anyhow::Result<()> {
    let mut h = Harness::start(HELD_READ_TIMEOUT).await?;
    let started = Instant::now();
    h.wait_reads(1).await?;
    // Bootstrap read R1 is held at revision 1. The overflow must not discard it.
    h.overflow().await?;
    let released = Instant::now();
    h.allow(1);
    h.expect_revision(1).await?;
    // The overflow hint starts the follow-up read at once, with no interval to wait.
    h.wait_reads(2).await?;
    let latency = h
        .arrival(1)
        .ok_or_else(|| anyhow::anyhow!("no second read arrival"))?
        .saturating_duration_since(released);
    anyhow::ensure!(
        latency <= Duration::from_millis(INTERVAL_MS) + PROMPT_SLACK,
        "first post-bootstrap read started {latency:?} after its release; {}",
        h.timeline()
    );

    // The live update lands while R2 is held: R2 captured revision 1, and the
    // overflow during R2 must not cost it. A follow-up then captures revision 2.
    h.set_revision(2);
    h.overflow().await?;
    h.allow(1);
    h.expect_revision(1).await?;
    h.wait_reads(3).await?;
    h.overflow().await?;
    h.allow(1);
    h.expect_revision(2).await?;
    // The overflow during R3 costs one follow-up, which confirms the same revision.
    h.wait_reads(4).await?;
    h.allow(1);
    h.expect_revision(2).await?;

    tokio::time::sleep(QUIET).await;
    let total = h.reads();
    anyhow::ensure!(
        total == 4,
        "expected 4 reads (bootstrap, three overflow follow-ups), saw {total}; {}",
        h.timeline()
    );
    let elapsed = started.elapsed();
    anyhow::ensure!(
        h.reported_reads() <= bound(elapsed) + 1,
        "reads {} exceed the interval bound {} for {elapsed:?}",
        h.reported_reads(),
        bound(elapsed) + 1
    );
    let reports = h.overflow_reports();
    let report_bound =
        1 + elapsed.as_millis() as u64 / service::REFUSAL_REPORT_INTERVAL.as_millis() as u64;
    anyhow::ensure!(
        (1..=report_bound).contains(&reports),
        "three overflow bursts logged {reports} lines in {elapsed:?}, bound {report_bound}"
    );
    Ok(())
}

/// A flood of refused deliveries is counted exactly, costs one held read, and is
/// logged at a bounded rate: the first refusal at once, then at most one line per
/// refusal-report interval. The bound is an upper bound, so a stalled host can only
/// make it looser.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_flood_is_counted_exactly_and_logged_at_a_bounded_rate() -> anyhow::Result<()> {
    const FLOOD: u64 = 5_000;
    let mut h = Harness::start(HELD_READ_TIMEOUT).await?;
    h.wait_reads(1).await?;
    h.allow(1);
    h.expect_revision(1).await?;
    let before = h.reads();
    let refused = h.refusals();
    let reports = h.reports();

    let started = Instant::now();
    // Paced, so the lane never overflows: this test is about refusals alone.
    h.hint_paced(FLOOD).await?;
    let elapsed = started.elapsed();
    // The first refusal starts one read, which is held. Later refusals only coalesce
    // into a pending flag, so the flood cannot start a second read.
    h.wait_reads(before + 1).await?;
    anyhow::ensure!(
        h.refusals() == refused + FLOOD,
        "refusals counted {} for {FLOOD} frames; {}",
        h.refusals() - refused,
        h.timeline()
    );
    anyhow::ensure!(
        h.lane_dropped() == 0,
        "paced flood must not overflow the lane; dropped {}",
        h.lane_dropped()
    );
    anyhow::ensure!(
        h.reads() == before + 1,
        "a refusal flood cost {} reads behind one held read; {}",
        h.reads() - before,
        h.timeline()
    );
    let logged = h.reports() - reports;
    let report_bound =
        1 + elapsed.as_millis() as u64 / service::REFUSAL_REPORT_INTERVAL.as_millis() as u64;
    anyhow::ensure!(
        (1..=report_bound).contains(&logged),
        "{FLOOD} refusals logged {logged} lines in {elapsed:?}, bound {report_bound}"
    );
    Ok(())
}

/// The connection moves while a read is held and status requests are in flight.
/// The held read belongs to the old connection, so it is discarded, and the new
/// connection converges on its own read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reconnect_while_a_read_is_held_converges_on_the_new_connection() -> anyhow::Result<()> {
    let mut h = Harness::start_on(Some(free_port()?), HELD_READ_TIMEOUT).await?;
    h.wait_reads(1).await?;
    h.allow(1);
    h.expect_revision(1).await?;
    let before = h.reads();

    // The held read on the first connection captures revision 2. The authority then
    // moves to revision 3, so the read on the new connection is distinguishable.
    h.set_revision(2);
    h.hint();
    h.wait_reads(before + 1).await?;
    h.set_revision(3);
    let flood = Flood::start(&h);
    wait_until("status replies in flight", || flood.replies() >= 5)
        .await
        .map_err(|error| anyhow::anyhow!("{error}; {}", h.timeline()))?;
    h.broker.bounce();
    flood.stop().await;

    // The held old-connection read is answered now. Its revision-2 result must not
    // be adopted, projected or cached. The serve loop takes reads in order, so the
    // next read on the new connection cannot be taken until this one is released.
    h.allow(1);
    h.wait_reads(before + 2).await?;
    h.allow(1);
    h.expect_revision(3).await?;
    tokio::time::sleep(QUIET).await;
    h.drain_projections();
    anyhow::ensure!(
        !h.log.iter().any(|line| line.starts_with("projected rev2 ")),
        "the old-generation revision-2 result was projected: {}",
        h.timeline()
    );
    Ok(())
}
