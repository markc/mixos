// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! backdrop on the Bus (AGENTS.md §5). One controller thread owns the
//! wallpaper engine, the command registry and the decoded images. Bus
//! deliveries and decode results arrive on its one bounded channel, so every
//! change is applied in order, answered once, and published on
//! `backdrop.command.executed`.
//!
//! - `backdrop.ping`, `backdrop.state`: queries, answered from the engine and
//!   the outputs the program has drawn. No output is needed.
//! - `backdrop.set`: intent. Its body is a complete document, the same as
//!   `backdrop.mx`. Every image it names is decoded again (so bytes replaced
//!   at the same path are picked up). When all have decoded, the document is
//!   stored (atomically, in the file it came from) and applied through the
//!   registry. Its reply comes when that has happened, or with the reason it
//!   did not. A refused set keeps the last applied backdrop on screen.
//!
//! `active` holds selected images, `candidate` holds a pending request and
//! `previous` retains outgoing images for a half-second crossfade. Candidates
//! apply together, or are dropped on supersession, decode or store refusal.
//!
//! Limits: one decode at a time, a queue of the pending request's images only
//! (at most four), and an inbox of 64 messages. Resident RGBA is bounded by
//! [`RESIDENT_BUDGET`]: a decode that would take active plus candidate over it
//! is refused.
//!
//! A change wakes the surfaces through their contexts. Slideshow and fade
//! deadlines drive timed waits on the inbox; a static wallpaper blocks there.

use crate::program::{Budget, FADE, Resident, Shared, View, Wakers, lock};
use citizen::{Delivery, Handle};
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use toolkit::command::{Command, Origin, Registry};
use wallpaper::decode::Decoded;
use wallpaper::engine::Schedule;
use wallpaper::schema::SlideSource;
use wallpaper::{Config, Decode, Document, Engine, Wallpaper};

/// The app's name: its default Bus service, its config directory and its
/// thread names.
pub const APP: &str = "backdrop";
/// The command a set runs: the one definition every change goes through.
pub const SET: &str = "backdrop.set";
pub const NEXT: &str = "backdrop.next";
/// The verbs this service answers, with their classes
/// (docs/spec/bus/verbs.conf.mix).
#[cfg(test)]
pub const VERBS: &[(&str, &str)] = &[
    ("backdrop.ping", "query"),
    ("backdrop.state", "query"),
    (SET, "intent"),
    (NEXT, "intent"),
];
/// Messages the controller's inbox holds before a sender waits.
const INBOX: usize = 64;
/// Decoded RGBA the controller may hold at once: the applied images and the
/// candidate's together. A full-size image (3840 × 3840 × 4 bytes) is 56.25 MiB,
/// so four applied plus four candidates (a complete replacement) is 450 MiB;
/// 512 MiB holds that with room to spare.
pub const RESIDENT_BUDGET: u64 = 512 * 1024 * 1024;

/// What the controller needs to start.
pub struct Setup {
    pub service: String,
    pub url: String,
    /// The document a set is stored in, if any.
    pub store: Option<PathBuf>,
    /// The configuration to start with (the file's, the command line's).
    pub config: Config,
    pub shared: Arc<Mutex<Shared>>,
    pub wakers: Wakers,
}

/// What reaches the controller's one channel.
enum Msg {
    Delivery(Delivery),
    BusStopped,
    Decoded {
        revision: u64,
        path: PathBuf,
        result: Result<Decoded, String>,
    },
    Prepared {
        revision: u64,
        key: Option<String>,
        result: Result<(Schedule, Decoded), String>,
    },
}

#[derive(Clone)]
enum Job {
    Load(u64, PathBuf),
    Resolve(u64, Option<String>, Wallpaper),
}

fn marker(key: &Option<String>) -> PathBuf {
    match key {
        None => PathBuf::from("slideshow:default"),
        Some(name) => PathBuf::from(format!("slideshow:output:{name}")),
    }
}

fn slides(config: &Config) -> Vec<(Option<String>, Wallpaper)> {
    std::iter::once((None, &config.default))
        .chain(
            config
                .outputs
                .iter()
                .map(|(key, wallpaper)| (Some(key.clone()), wallpaper)),
        )
        .filter(|(_, wallpaper)| matches!(wallpaper, Wallpaper::Slideshow { .. }))
        .map(|(key, wallpaper)| (key, wallpaper.clone()))
        .collect()
}

/// Only the decode worker calls this: folder traversal and image loading never
/// run on the controller or the Wayland thread.
fn prepare(wallpaper: Wallpaper, revision: u64) -> Result<(Schedule, Decoded), String> {
    let Wallpaper::Slideshow {
        source,
        interval_s,
        shuffle,
        ..
    } = wallpaper
    else {
        return Err("not a slideshow source".into());
    };
    let files = match source {
        SlideSource::Folder(path) => wallpaper::decode::list_folder(&path)?,
        SlideSource::Images(paths) => paths,
    };
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(revision, |d| d.as_nanos() as u64 ^ revision);
    let schedule = Schedule::new(files, interval_s, shuffle, seed, Duration::ZERO)?;
    let image = wallpaper::decode::load(schedule.current())?;
    Ok((schedule, image))
}

fn resolved(config: &Config, schedules: &BTreeMap<Option<String>, Schedule>) -> Config {
    let resolve = |key: Option<String>, wallpaper: &Wallpaper| match wallpaper {
        Wallpaper::Slideshow { fallback, fit, .. } => {
            schedules
                .get(&key)
                .map_or(Wallpaper::Theme, |s| Wallpaper::Image {
                    path: s.current().to_path_buf(),
                    fallback: *fallback,
                    fit: *fit,
                })
        }
        other => other.clone(),
    };
    Config {
        default: resolve(None, &config.default),
        outputs: config
            .outputs
            .iter()
            .map(|(key, wallpaper)| (key.clone(), resolve(Some(key.clone()), wallpaper)))
            .collect(),
    }
}

/// Who is owed an answer for a revision.
enum Owed {
    /// The start-up document: nothing to answer.
    Startup,
    /// A Bus request, by its delivery id.
    Bus(u64),
    Next(u64),
    Timer,
}

/// Why a document could not be stored.
enum Stored {
    /// Not stored; the applied configuration stays.
    Refused(String),
    /// Stored, but the write's durability is unconfirmed (after a rename).
    Unconfirmed(String),
}

/// Start the Bus connection and the controller. A Bus that does not register
/// is an error: the wallpaper does not run as an unregistered second copy.
pub fn start(setup: Setup) -> Result<(), String> {
    let (results, inbox) = mpsc::sync_channel::<Msg>(INBOX);
    let (handle, deliveries) = citizen::start(APP, &setup.service, &setup.url)
        .map_err(|error| format!("Bus registration as {:?}: {error}", setup.service))?;
    let forward = results.clone();
    std::thread::Builder::new()
        .name("backdrop-bus".into())
        .spawn(move || {
            let mut deliveries = deliveries;
            futures::executor::block_on(async {
                while let Some(delivery) = deliveries.next().await {
                    // A full inbox waits here, on this thread, not on the Bus runtime.
                    if forward.send(Msg::Delivery(delivery)).is_err() {
                        break;
                    }
                }
            });
            let _ = forward.send(Msg::BusStopped);
        })
        .map_err(|e| format!("bus thread: {e}"))?;
    let Setup {
        service,
        url: _,
        store,
        config,
        shared,
        wakers,
    } = setup;
    // The registry is not `Send`: the controller builds it on its own thread.
    std::thread::Builder::new()
        .name("backdrop-control".into())
        .spawn(move || {
            let controller = Controller {
                handle: Some(handle),
                service,
                store,
                engine: Engine::new(),
                registry: registry(),
                active: BTreeMap::new(),
                candidate: BTreeMap::new(),
                serial: 0,
                memory: Budget::new(RESIDENT_BUDGET),
                queue: VecDeque::new(),
                decoding: false,
                owed: BTreeMap::new(),
                schedules: BTreeMap::new(),
                pending_schedules: None,
                advancing: BTreeSet::new(),
                display: Config::default(),
                previous: BTreeMap::new(),
                previous_config: None,
                fade_started: None,
                started: Instant::now(),
                shared,
                wakers,
                results,
            };
            controller.run(config, inbox);
        })
        .map(|_| ())
        .map_err(|e| format!("control thread: {e}"))
}

/// The command registry: every change to the applied backdrop is one of its
/// commands. Its run applies the request that is ready.
fn registry() -> Registry<Engine> {
    let mut registry = Registry::new();
    registry.add(Command {
        id: SET,
        label: "backdrop-set",
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: |engine: &Engine| engine.ready(),
        run: |engine: &mut Engine| {
            engine.apply();
        },
    });
    registry.add(Command {
        id: NEXT,
        label: "backdrop-next",
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: |engine: &Engine| engine.ready() && !slides(engine.applied()).is_empty(),
        run: |engine: &mut Engine| {
            engine.apply();
        },
    });
    registry
}

struct Controller {
    handle: Option<Handle>,
    service: String,
    store: Option<PathBuf>,
    engine: Engine,
    registry: Registry<Engine>,
    /// The applied document's decoded images: on screen.
    active: BTreeMap<PathBuf, Arc<Resident>>,
    /// The pending request's decoded images: not yet applied.
    candidate: BTreeMap<PathBuf, Arc<Resident>>,
    serial: u64,
    memory: Arc<Budget>,
    /// The pending request's decodes still to run, in order: one at a time.
    queue: VecDeque<Job>,
    decoding: bool,
    owed: BTreeMap<u64, Owed>,
    schedules: BTreeMap<Option<String>, Schedule>,
    pending_schedules: Option<BTreeMap<Option<String>, Schedule>>,
    advancing: BTreeSet<Option<String>>,
    display: Config,
    previous: BTreeMap<PathBuf, Arc<Resident>>,
    previous_config: Option<Config>,
    fade_started: Option<Instant>,
    started: Instant,
    shared: Arc<Mutex<Shared>>,
    wakers: Wakers,
    results: mpsc::SyncSender<Msg>,
}

impl Controller {
    fn run(mut self, config: Config, inbox: mpsc::Receiver<Msg>) {
        self.stage(config, Owed::Startup);
        loop {
            self.tick();
            let received = match self.next_deadline() {
                Some(wait) => inbox.recv_timeout(wait),
                None => inbox
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            };
            let msg = match received {
                Ok(msg) => msg,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match msg {
                Msg::Delivery(Delivery::Command { id, verb, body }) => {
                    self.command(id, &verb, &body)
                }
                Msg::Delivery(Delivery::Disconnected) => {
                    eprintln!("backdrop: Bus disconnected; native supervision is reconnecting");
                }
                Msg::Delivery(Delivery::Connected) => {
                    eprintln!("backdrop: Bus connected");
                }
                Msg::Delivery(_) => {}
                Msg::BusStopped => {
                    eprintln!("backdrop: native Bus delivery stream stopped");
                    std::process::exit(1);
                }
                Msg::Decoded {
                    revision,
                    path,
                    result,
                } => self.decoded(revision, path, result),
                Msg::Prepared {
                    revision,
                    key,
                    result,
                } => self.prepared(revision, key, result),
            }
        }
    }

    fn command(&mut self, id: u64, verb: &str, body: &str) {
        match verb {
            "backdrop.ping" => self.reply(
                id,
                0,
                json!({"schema": "backdrop.v1", "service": self.service}),
            ),
            "backdrop.state" => {
                let state = self.state();
                self.reply(id, 0, state);
            }
            SET => match parse_document(body) {
                Ok(config) => self.stage(config, Owed::Bus(id)),
                Err(message) => self.reply(id, 10, refusal("ARGUMENT", message)),
            },
            NEXT => match parse_next(body) {
                Err(reason) => self.reply(id, 10, refusal("ARGUMENT", reason)),
                Ok(()) if self.schedules.is_empty() => self.reply(
                    id,
                    10,
                    refusal("NON_SLIDESHOW", "the applied backdrop has no slideshow"),
                ),
                Ok(()) if self.engine.pending().is_some() => {
                    self.reply(id, 10, refusal("BUSY", "a set or advance is pending"))
                }
                Ok(()) => self.advance(self.schedules.keys().cloned().collect(), Owed::Next(id)),
            },
            other => self.reply(id, 10, refusal("UNKNOWN", format!("no verb {other:?}"))),
        }
    }

    /// Number a request and queue all its images for decode. Every image is
    /// decoded again, even one already resident: an explicit set repairs bytes
    /// replaced at the same path. A superseded request's partial candidates go
    /// at once.
    fn stage(&mut self, config: Config, owed: Owed) {
        self.finish_fade();
        let sources: BTreeMap<_, _> = slides(&config)
            .into_iter()
            .map(|(key, wallpaper)| (marker(&key), (key, wallpaper)))
            .collect();
        let paths = config
            .images()
            .into_iter()
            .map(Path::to_path_buf)
            .chain(sources.keys().cloned())
            .collect();
        let staged = self.engine.stage_images(config, paths, |_| false);
        if let Some(old) = staged.superseded {
            let old = self.owed.remove(&old);
            self.answer(
                old,
                10,
                refusal(
                    "SUPERSEDED",
                    format!("superseded by revision {}", staged.revision),
                ),
            );
        }
        self.candidate.clear();
        self.pending_schedules = Some(BTreeMap::new());
        self.advancing.clear();
        let revision = staged.revision;
        self.owed.insert(revision, owed);
        self.queue.clear();
        self.queue.extend(
            staged
                .jobs
                .into_iter()
                .map(|path| match sources.get(&path) {
                    Some((key, wallpaper)) => {
                        Job::Resolve(revision, key.clone(), wallpaper.clone())
                    }
                    None => Job::Load(revision, path),
                }),
        );
        if self.engine.ready() {
            self.complete();
        } else {
            self.next_decode();
        }
    }

    fn advance(&mut self, keys: BTreeSet<Option<String>>, owed: Owed) {
        if self.engine.pending().is_some() || keys.is_empty() {
            return;
        }
        self.finish_fade();
        let now = self.started.elapsed();
        let mut schedules = self.schedules.clone();
        let mut refresh = BTreeSet::new();
        for key in &keys {
            if let Some(schedule) = schedules.get_mut(key) {
                schedule.advance(now);
                refresh.insert(schedule.current().to_path_buf());
            }
        }
        let config = self.engine.applied().clone();
        let display = resolved(&config, &schedules);
        let paths: Vec<_> = display
            .images()
            .into_iter()
            .map(Path::to_path_buf)
            .collect();
        self.candidate = self
            .active
            .iter()
            .filter(|(path, _)| paths.contains(path) && !refresh.contains(*path))
            .map(|(path, resident)| (path.clone(), resident.clone()))
            .collect();
        let staged = self.engine.stage_images(config, paths, |path| {
            self.active.contains_key(path) && !refresh.contains(path)
        });
        self.pending_schedules = Some(schedules);
        self.advancing = keys;
        self.owed.insert(staged.revision, owed);
        self.queue = staged
            .jobs
            .into_iter()
            .map(|path| Job::Load(staged.revision, path))
            .collect();
        if self.engine.ready() {
            self.complete();
        } else {
            self.next_decode();
        }
    }

    /// Blocking until the nearest real deadline; pending decode suppresses
    /// schedule wakes, but not fade cleanup. A static image has no deadline.
    fn next_deadline(&self) -> Option<Duration> {
        let fade = self
            .fade_started
            .map(|start| FADE.saturating_sub(start.elapsed()));
        let slide = if self.engine.pending().is_none() {
            self.schedules
                .values()
                .filter_map(Schedule::deadline)
                .min()
                .map(|at| at.saturating_sub(self.started.elapsed()))
        } else {
            None
        };
        fade.into_iter().chain(slide).min()
    }

    fn tick(&mut self) {
        if self
            .fade_started
            .is_some_and(|start| start.elapsed() >= FADE)
        {
            self.finish_fade();
        }
        if self.engine.pending().is_none() {
            let now = self.started.elapsed();
            let due = self
                .schedules
                .iter()
                .filter(|(_, schedule)| schedule.due(now))
                .map(|(key, _)| key.clone())
                .collect();
            self.advance(due, Owed::Timer);
        }
    }

    fn finish_fade(&mut self) {
        if self.fade_started.take().is_some() {
            self.previous.clear();
            self.previous_config = None;
            self.publish();
        }
    }

    /// Start the next decode, if none is running.
    fn next_decode(&mut self) {
        if self.decoding {
            return;
        }
        let Some(job) = self.queue.pop_front() else {
            return;
        };
        self.decoding = true;
        let results = self.results.clone();
        let backup = job.clone();
        let spawned = std::thread::Builder::new()
            .name("backdrop-decode".into())
            .spawn(move || {
                let msg = match job {
                    Job::Load(revision, path) => {
                        let result = wallpaper::decode::load(&path);
                        Msg::Decoded {
                            revision,
                            path,
                            result,
                        }
                    }
                    Job::Resolve(revision, key, wallpaper) => Msg::Prepared {
                        revision,
                        key,
                        result: prepare(wallpaper, revision),
                    },
                };
                let _ = results.send(msg);
            });
        if let Err(error) = spawned {
            let reason = format!("decoder thread: {error}");
            match backup {
                Job::Load(revision, path) => self.decoded(revision, path, Err(reason)),
                Job::Resolve(revision, key, _) => self.prepared(revision, key, Err(reason)),
            }
        }
    }

    fn decoded(&mut self, revision: u64, path: PathBuf, result: Result<Decoded, String>) {
        self.decoding = false;
        let verdict = match &result {
            Ok(image) if self.resident_bytes() + image.bytes() > RESIDENT_BUDGET => Err(format!(
                "{}: {} bytes resident would exceed the {RESIDENT_BUDGET}-byte budget",
                path.display(),
                self.resident_bytes() + image.bytes()
            )),
            other => other.as_ref().map(|_| ()).map_err(Clone::clone),
        };
        match self.engine.decoded(revision, &path, verdict) {
            // A stale completion is dropped here, with its pixels.
            Decode::Superseded => {}
            Decode::Waiting => self.keep(path, result),
            Decode::Ready => {
                self.keep(path, result);
                self.complete();
            }
            Decode::Failed(message) => {
                self.failed(revision, message);
            }
        }
        self.next_decode();
    }

    fn prepared(
        &mut self,
        revision: u64,
        key: Option<String>,
        result: Result<(Schedule, Decoded), String>,
    ) {
        self.decoding = false;
        let verdict = match &result {
            Ok((_, image)) if self.resident_bytes() + image.bytes() > RESIDENT_BUDGET => Err(
                format!("resident image budget exceeds {RESIDENT_BUDGET} bytes"),
            ),
            other => other.as_ref().map(|_| ()).map_err(Clone::clone),
        };
        match self.engine.decoded(revision, &marker(&key), verdict) {
            Decode::Superseded => {}
            Decode::Failed(message) => self.failed(revision, message),
            outcome => {
                if let Ok((schedule, image)) = result {
                    self.keep(schedule.current().to_path_buf(), Ok(image));
                    if let Some(pending) = &mut self.pending_schedules {
                        pending.insert(key, schedule);
                    }
                }
                if outcome == Decode::Ready {
                    self.complete();
                }
            }
        }
        self.next_decode();
    }

    fn failed(&mut self, revision: u64, message: String) {
        self.candidate.clear();
        self.queue.clear();
        self.pending_schedules = None;
        let now = self.started.elapsed();
        for key in std::mem::take(&mut self.advancing) {
            if let Some(schedule) = self.schedules.get_mut(&key) {
                schedule.defer(now);
            }
        }
        let owed = self.owed.remove(&revision);
        if matches!(owed, Some(Owed::Startup)) {
            eprintln!("backdrop: {message}; the session theme stays");
        }
        self.answer(owed, 10, refusal("IMAGE", message));
    }

    /// Bytes of decoded RGBA held: the applied images and the candidate's.
    fn resident_bytes(&self) -> u64 {
        self.memory.used()
    }

    /// Keep a decoded image as a candidate of the request it was decoded for.
    fn keep(&mut self, path: PathBuf, result: Result<Decoded, String>) {
        if let Ok(image) = result {
            let reservation = self
                .memory
                .reserve(image.bytes())
                .expect("the single controller checked the resident budget before keeping pixels");
            self.serial += 1;
            self.candidate.insert(
                path,
                Arc::new(Resident {
                    serial: self.serial,
                    image,
                    _resident_bytes: Some(Arc::new(reservation)),
                }),
            );
        }
    }

    /// Apply the ready request: store it (for a Bus set), run its command,
    /// make its candidates the active images, wake the surfaces, publish the
    /// run and answer.
    fn complete(&mut self) {
        let Some(revision) = self.engine.pending() else {
            return;
        };
        let owed = self.owed.remove(&revision);
        let mut unconfirmed = None;
        if matches!(owed, Some(Owed::Bus(_)))
            && let (Some(store), Some(config)) =
                (self.store.clone(), self.engine.pending_config().cloned())
        {
            match persist(&store, &config) {
                Ok(()) => {}
                Err(Stored::Refused(reason)) => {
                    self.candidate.clear();
                    self.pending_schedules = None;
                    self.queue.clear();
                    self.engine.refuse(revision, reason.clone());
                    self.answer(owed, 10, refusal("STORE", reason));
                    return;
                }
                Err(Stored::Unconfirmed(reason)) => unconfirmed = Some(reason),
            }
        }
        let applied = match owed {
            Some(Owed::Bus(_)) => self
                .registry
                .execute(SET, &mut self.engine, Origin::Bus)
                .map_err(|e| e.to_string()),
            Some(Owed::Next(_)) => self
                .registry
                .execute(NEXT, &mut self.engine, Origin::Bus)
                .map_err(|e| e.to_string()),
            _ => {
                self.engine.apply();
                Ok(())
            }
        };
        if applied.is_ok() {
            let now = self.started.elapsed();
            let mut schedules = self.pending_schedules.take().unwrap_or_default();
            for (key, schedule) in &mut schedules {
                if self.advancing.is_empty() || self.advancing.contains(key) {
                    schedule.defer(now);
                }
            }
            let display = resolved(self.engine.applied(), &schedules);
            let fading = !self.advancing.is_empty() && display != self.display;
            let old = std::mem::replace(&mut self.active, std::mem::take(&mut self.candidate));
            if fading {
                self.previous = old;
                self.previous_config = Some(self.display.clone());
                self.fade_started = Some(Instant::now());
            }
            self.display = display;
            self.schedules = schedules;
        } else {
            self.candidate.clear();
            self.pending_schedules = None;
            self.engine.refuse(
                revision,
                "command registry refused the ready request".into(),
            );
        }
        self.advancing.clear();
        self.queue.clear();
        self.publish();
        self.journal();
        match (owed, applied, unconfirmed) {
            (Some(Owed::Next(id)), Ok(()), _) => self.reply(id, 0, self.state()),
            (Some(Owed::Next(id)), Err(reason), _) => {
                self.reply(id, 10, refusal("INTERNAL", reason))
            }
            (Some(Owed::Bus(id)), Ok(()), None) => self.reply(id, 0, self.change()),
            (Some(Owed::Bus(id)), Ok(()), Some(reason)) => self.reply(
                id,
                10,
                refusal("STORE", format!("applied; not confirmed stored: {reason}")),
            ),
            (Some(Owed::Bus(id)), Err(reason), _) => {
                self.reply(id, 10, refusal("INTERNAL", reason));
            }
            _ => {}
        }
    }

    /// Hand the surfaces the applied configuration and its images, and wake
    /// every surface's context to draw it.
    fn publish(&self) {
        let view = Arc::new(View {
            revision: self.engine.status().active,
            config: self.display.clone(),
            images: self.active.clone(),
            previous: self.previous.clone(),
            previous_config: self.previous_config.clone(),
            fade_started: self.fade_started,
        });
        lock(&self.shared).view = view;
        let contexts: Vec<_> = lock(&self.wakers)
            .iter()
            .map(|(_, context)| context.clone())
            .collect();
        for context in contexts {
            context.request_repaint();
        }
    }

    /// Publish the runs the registry recorded, on `<service>.command.executed`.
    fn journal(&self) {
        let Some(handle) = &self.handle else {
            return;
        };
        let (runs, lost) = self.registry.take_executed();
        if runs.is_empty() && lost == 0 {
            return;
        }
        let topic = toolkit::executed::topic(handle.service());
        let bodies = runs.iter().map(toolkit::executed::Executed::body).collect();
        handle.publish(&topic, bodies, lost);
    }

    fn reply(&self, id: u64, rc: u8, body: Value) {
        if let Some(handle) = &self.handle {
            handle.reply(id, rc, body);
        }
    }

    fn answer(&self, owed: Option<Owed>, rc: u8, body: Value) {
        if let Some(Owed::Bus(id) | Owed::Next(id)) = owed {
            self.reply(id, rc, body);
        }
    }

    fn store_text(&self) -> Option<String> {
        self.store.as_ref().map(|path| path.display().to_string())
    }

    /// The reply to an applied set.
    fn change(&self) -> Value {
        let status = self.engine.status();
        json!({
            "requested": status.requested,
            "active": status.active,
            "config_file": self.store_text(),
        })
    }

    /// The state a query reports.
    fn state(&self) -> Value {
        let status = self.engine.status();
        let outputs: Vec<Value> = lock(&self.shared)
            .outputs
            .iter()
            .map(|(name, output)| {
                json!({
                    "name": name,
                    "width": output.width,
                    "height": output.height,
                    "scale": output.scale,
                    "shows": output.shows,
                    "presented_revision": output.revision,
                    "error": output.error,
                    "fit": wallpaper_fit(self.display.for_output(Some(name))),
                })
            })
            .collect();
        json!({
            "schema": "backdrop.v1",
            "service": self.service,
            "requested": status.requested,
            "active": status.active,
            "pending": status.pending,
            "error": status.error,
            "config": status.config,
            "config_file": self.store_text(),
            "outputs": outputs,
            "fit": wallpaper_fit(&self.display.default),
            "slideshows": self.schedules.iter().map(|(key, schedule)| {
                let fading = self.previous_config.as_ref().is_some_and(|old|
                    old.for_output(key.as_deref()) != self.display.for_output(key.as_deref()));
                json!({"output": key, "current_path": schedule.current(),
                    "index": schedule.current_index, "count": schedule.files.len(),
                    "interval_s": schedule.interval_s, "shuffle": schedule.shuffle,
                    "pending": self.engine.pending().is_some(), "fading": fading})
            }).collect::<Vec<_>>(),
        })
    }
}

fn wallpaper_fit(wallpaper: &Wallpaper) -> Option<wallpaper::fit::Fit> {
    match wallpaper {
        Wallpaper::Image { fit, .. } | Wallpaper::Slideshow { fit, .. } => Some(*fit),
        _ => None,
    }
}

fn parse_next(body: &str) -> Result<(), String> {
    if body.len() as u64 > crate::MAX_DOCUMENT {
        return Err("body: too large".into());
    }
    match serde_json::from_str::<Value>(body).map_err(|e| format!("body: {e}"))? {
        Value::Object(fields) if fields.is_empty() => Ok(()),
        _ => Err("body: expected the empty object {}".into()),
    }
}

/// A Bus body as a validated document: the same rules as the file's.
fn parse_document(body: &str) -> Result<Config, String> {
    if body.len() as u64 > crate::MAX_DOCUMENT {
        return Err(format!("body: over {} bytes", crate::MAX_DOCUMENT));
    }
    let document: Document = serde_json::from_str(body).map_err(|e| format!("body: {e}"))?;
    Config::from_document(document).map_err(|e| e.to_string())
}

/// A refusal in the form every Bus service uses: `error_code` and `message`.
fn refusal(code: &str, message: impl Into<String>) -> Value {
    json!({"error_code": code, "message": message.into()})
}

/// Store `config` at `store`, durably: the directory is provisioned, then the
/// file is replaced atomically.
fn persist(store: &Path, config: &Config) -> Result<(), Stored> {
    let text = config
        .to_conf_text()
        .map_err(|e| Stored::Refused(e.to_string()))?;
    if text.len() as u64 > crate::MAX_DOCUMENT {
        return Err(Stored::Refused(format!(
            "stored document: over {} bytes",
            crate::MAX_DOCUMENT
        )));
    }
    let dir = store
        .parent()
        .ok_or_else(|| Stored::Refused(format!("{}: no directory", store.display())))?;
    config::atomic::create_directory(dir)
        .map_err(|e| Stored::Refused(format!("{}: {e}", dir.display())))?;
    config::atomic::replace(store, text.as_bytes()).map_err(|e| {
        if e.may_have_replaced {
            Stored::Unconfirmed(e.to_string())
        } else {
            Stored::Refused(e.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wallpaper::Wallpaper;

    const REGISTRY: &str = include_str!("../../../docs/spec/bus/verbs.conf.mix");

    /// A field of one registry line: `key:"value"`.
    fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
        let start = line.find(&format!("{key}:\""))? + key.len() + 2;
        line[start..].split('"').next()
    }

    /// A controller with no Bus and no window. Its decoder threads send into a
    /// receiver that is dropped, so their results are never applied; the tests
    /// feed decode results in by hand.
    fn controller() -> Controller {
        let (results, _inbox) = mpsc::sync_channel(INBOX);
        Controller {
            handle: None,
            service: "test".into(),
            store: None,
            engine: Engine::new(),
            registry: registry(),
            active: BTreeMap::new(),
            candidate: BTreeMap::new(),
            serial: 0,
            memory: Budget::new(RESIDENT_BUDGET),
            queue: VecDeque::new(),
            decoding: false,
            owed: BTreeMap::new(),
            schedules: BTreeMap::new(),
            pending_schedules: None,
            advancing: BTreeSet::new(),
            display: Config::default(),
            previous: BTreeMap::new(),
            previous_config: None,
            fade_started: None,
            started: Instant::now(),
            shared: Arc::new(Mutex::new(Shared::default())),
            wakers: Arc::new(Mutex::new(Vec::new())),
            results,
        }
    }

    /// A document with one output per path, each an image.
    fn images(paths: &[&str]) -> Config {
        let mut config = Config::default();
        for (i, path) in paths.iter().enumerate() {
            config.outputs.insert(
                format!("o{i}"),
                Wallpaper::Image {
                    path: PathBuf::from(path),
                    fallback: None,
                    fit: wallpaper::fit::Fit::Fill,
                },
            );
        }
        config
    }

    fn decoded_ok() -> Result<Decoded, String> {
        Ok(Decoded {
            size: [1, 1],
            original_size: [1, 1],
            rgba: image::RgbaImage::new(1, 1),
        })
    }

    #[test]
    fn the_registry_lists_exactly_the_verbs_answered_with_their_classes() {
        let mut registered: Vec<(String, String)> = REGISTRY
            .lines()
            .filter(|line| line.contains("owner:\"backdrop\""))
            .map(|line| {
                (
                    field(line, "name").unwrap_or_default().to_owned(),
                    field(line, "class").unwrap_or_default().to_owned(),
                )
            })
            .collect();
        registered.sort();
        let mut answered: Vec<(String, String)> = VERBS
            .iter()
            .map(|(name, class)| ((*name).to_owned(), (*class).to_owned()))
            .collect();
        answered.sort();
        assert_eq!(registered, answered);
    }

    #[test]
    fn set_and_next_are_commands_and_set_runs_through_the_registry() {
        let registry = registry();
        assert_eq!(registry.iter().count(), 2);
        let mut engine = Engine::new();
        engine.stage(Config::default(), |_| false);
        registry
            .execute(SET, &mut engine, Origin::Bus)
            .expect("a ready set runs");
        assert_eq!(engine.status().active, 1);
        let (runs, lost) = registry.take_executed();
        assert_eq!((runs.len(), lost), (1, 0));
        assert_eq!(runs[0].id, SET);
        assert_eq!(runs[0].origin, Origin::Bus);
        assert!(
            registry.execute(SET, &mut engine, Origin::Bus).is_err(),
            "nothing is ready, so the command is refused and not recorded"
        );
        assert!(registry.take_executed().0.is_empty());
    }

    #[test]
    fn an_oversized_bus_document_is_refused_before_parsing() {
        let body = " ".repeat(crate::MAX_DOCUMENT as usize + 1);
        assert!(parse_document(&body).unwrap_err().contains("over"));
    }

    #[test]
    fn a_bus_body_is_validated_before_any_change() {
        assert!(parse_document(r##"{"schema":1,"default":{"colour":"#102030"}}"##).is_ok());
        assert!(parse_document(r#"{"schema":1,"default":{"colour":"red"}}"#).is_err());
        assert!(parse_document(r#"{"schema":1,"nope":1}"#).is_err());
        assert!(parse_document("{}").is_err());
    }

    #[test]
    fn candidates_are_dropped_on_supersession_and_stale_results_never_land() {
        let mut c = controller();
        c.stage(images(&["/w/a.png", "/w/b.png"]), Owed::Startup);
        c.decoded(1, PathBuf::from("/w/a.png"), decoded_ok());
        assert_eq!(c.candidate.len(), 1, "a decoded candidate is held");
        assert!(c.active.is_empty(), "nothing is applied until all decode");
        // A newer request supersedes it: its partial candidates go at once.
        c.stage(images(&["/w/c.png"]), Owed::Startup);
        assert!(c.candidate.is_empty());
        // The superseded request's late result is dropped, not kept.
        c.decoded(1, PathBuf::from("/w/b.png"), decoded_ok());
        assert!(c.candidate.is_empty(), "a stale completion never lands");
        assert!(c.active.is_empty());
    }

    #[test]
    fn an_explicit_set_decodes_a_resident_path_again_and_a_failure_keeps_the_last_good() {
        let mut c = controller();
        let path = PathBuf::from("/w/a.png");
        c.stage(images(&["/w/a.png"]), Owed::Startup);
        c.decoded(1, path.clone(), decoded_ok());
        assert_eq!(c.active.len(), 1, "the first request applied");
        assert!(c.candidate.is_empty(), "the commit emptied the candidates");
        let good = c.active[&path].serial;

        // The same path again: decoded again, not reused from the active map.
        c.stage(images(&["/w/a.png"]), Owed::Startup);
        assert_eq!(c.engine.pending(), Some(2));
        assert!(!c.engine.ready(), "the path is decoded again");
        // The refreshed bytes are corrupt: the last good image stays active.
        c.decoded(2, path.clone(), Err("corrupt".into()));
        assert!(c.candidate.is_empty());
        assert_eq!(
            c.active[&path].serial, good,
            "the applied image is untouched"
        );
        let status = c.engine.status();
        assert_eq!((status.requested, status.active), (2, 1));
        assert!(status.error.is_some());
    }

    #[test]
    fn a_failed_decode_drops_the_queued_jobs_too() {
        let mut c = controller();
        c.stage(images(&["/w/a.png", "/w/b.png", "/w/c.png"]), Owed::Startup);
        assert!(!c.queue.is_empty(), "the other images wait in the queue");
        c.decoded(1, PathBuf::from("/w/a.png"), Err("corrupt".into()));
        assert!(
            c.queue.is_empty(),
            "no decode is wasted on a failed request"
        );
        assert!(c.candidate.is_empty());
        assert!(!c.decoding);
    }

    /// A full-size image's accounting (3840 × 3840 × 4 bytes). The pixel buffer
    /// is 1 × 1: only the size fields are counted, so no 56 MiB is allocated.
    fn full_size() -> Result<Decoded, String> {
        Ok(Decoded {
            size: [3840, 3840],
            original_size: [3840, 3840],
            rgba: image::RgbaImage::new(1, 1),
        })
    }

    #[test]
    fn a_complete_replacement_of_four_full_size_images_fits_the_budget() {
        const MIB: u64 = 1024 * 1024;
        let mut c = controller();
        let paths = ["/w/1.png", "/w/2.png", "/w/3.png", "/w/4.png"];
        // Four applied full-size images: 225 MiB.
        c.stage(images(&paths), Owed::Startup);
        for path in paths {
            c.decoded(1, PathBuf::from(path), full_size());
        }
        assert_eq!(c.active.len(), 4);
        assert_eq!(c.resident_bytes(), 4 * 3840 * 3840 * 4);
        // Four candidates on top, the same size: 450 MiB, within 512 MiB.
        let fresh = ["/w/5.png", "/w/6.png", "/w/7.png", "/w/8.png"];
        c.stage(images(&fresh), Owed::Startup);
        for path in fresh {
            c.decoded(2, PathBuf::from(path), full_size());
        }
        assert_eq!(c.active.len(), 4, "the full replacement applied");
        assert!(c.active.contains_key(&PathBuf::from("/w/8.png")));
        // While the last good images are held, a candidate that would take the
        // total over 512 MiB is refused: 4 × 56.25 MiB + 300 MiB > 512 MiB.
        c.stage(images(&["/w/9.png"]), Owed::Startup);
        c.decoded(
            3,
            PathBuf::from("/w/9.png"),
            Ok(Decoded {
                size: [5_000, 15_625],
                original_size: [5_000, 15_625],
                rgba: image::RgbaImage::new(1, 1),
            }),
        );
        assert!(
            c.engine
                .status()
                .error
                .as_deref()
                .is_some_and(|e| e.contains("budget"))
        );
        assert_eq!(c.active.len(), 4, "the last good images are still applied");
        assert!(c.resident_bytes() <= 512 * MIB);
    }

    #[test]
    fn the_resident_budget_refuses_a_decode_that_would_overflow_it() {
        let mut c = controller();
        c.stage(images(&["/w/a.png"]), Owed::Startup);
        // One byte-row over the budget (8193 × 16384 × 4 bytes).
        let big = Decoded {
            size: [16_384, 8_193],
            original_size: [16_384, 8_193],
            rgba: image::RgbaImage::new(1, 1),
        };
        c.decoded(1, PathBuf::from("/w/a.png"), Ok(big));
        assert!(
            c.candidate.is_empty(),
            "a result over the budget is refused"
        );
        assert!(
            c.engine
                .status()
                .error
                .as_deref()
                .is_some_and(|e| e.contains("budget"))
        );
    }

    fn slide_config() -> Config {
        Config::from_text("schema: 1\ndefault: { slideshow: { images: [\"/w/a.png\", \"/w/b.png\"], interval_s: 5 } }").unwrap()
    }

    fn start_slide(c: &mut Controller) {
        c.stage(slide_config(), Owed::Startup);
        let schedule = Schedule::new(
            vec!["/w/a.png".into(), "/w/b.png".into()],
            5,
            false,
            1,
            Duration::ZERO,
        )
        .unwrap();
        c.prepared(1, None, Ok((schedule, decoded_ok().unwrap())));
        assert_eq!(c.engine.status().active, 1);
    }

    #[test]
    fn next_body_is_strict_and_static_images_have_no_timer_or_next_run() {
        assert!(parse_next("{}").is_ok());
        for body in ["[]", "null", "{\"x\":1}", "broken"] {
            assert!(parse_next(body).is_err());
        }
        let mut c = controller();
        c.stage(images(&["/w/a.png"]), Owed::Startup);
        c.decoded(1, "/w/a.png".into(), decoded_ok());
        assert_eq!(c.next_deadline(), None);
        c.command(7, NEXT, "{}");
        assert_eq!(c.engine.status().requested, 1);
        assert!(c.registry.take_executed().0.is_empty());
    }

    #[test]
    fn next_applies_after_decode_and_records_the_bus_command_with_full_state() {
        let mut c = controller();
        start_slide(&mut c);
        c.command(7, NEXT, "{}");
        assert_eq!(c.engine.pending(), Some(2));
        assert_eq!(c.schedules[&None].current(), Path::new("/w/a.png"));
        assert_eq!(
            c.next_deadline(),
            None,
            "a decode waits for a completion, not its old deadline"
        );
        c.decoded(2, "/w/b.png".into(), decoded_ok());
        assert_eq!(c.schedules[&None].current(), Path::new("/w/b.png"));
        let (runs, lost) = c.registry.take_executed();
        assert_eq!((runs.len(), lost), (1, 0));
        assert_eq!(runs[0].id, NEXT);
        assert_eq!(runs[0].origin, Origin::Bus);
        let state = c.state();
        assert_eq!(state["slideshows"][0]["index"], 1);
        assert_eq!(state["slideshows"][0]["count"], 2);
        assert_eq!(state["fit"], "fill");
        assert!(state["config"]["default"]["slideshow"].is_object());
        assert!(c.fade_started.is_some());
    }

    #[test]
    fn a_failed_next_keeps_the_selection_and_schedules_a_future_retry() {
        let mut c = controller();
        start_slide(&mut c);
        let serial = c.active[Path::new("/w/a.png")].serial;
        c.command(7, NEXT, "{}");
        c.decoded(2, "/w/b.png".into(), Err("corrupt".into()));
        assert_eq!(c.engine.status().active, 1);
        assert_eq!(c.active[Path::new("/w/a.png")].serial, serial);
        assert_eq!(c.schedules[&None].current(), Path::new("/w/a.png"));
        assert!(!c.schedules[&None].due(c.started.elapsed()));
        assert!(c.engine.status().error.is_some());
        assert!(c.registry.take_executed().0.is_empty());
        assert!(c.candidate.is_empty());
    }

    #[test]
    fn a_set_supersedes_a_pending_next_and_its_late_completion() {
        let mut c = controller();
        start_slide(&mut c);
        c.command(7, NEXT, "{}");
        c.stage(Config::default(), Owed::Startup);
        c.decoded(2, "/w/b.png".into(), decoded_ok());
        assert_eq!(c.engine.status().active, 3);
        assert_eq!(c.engine.applied(), &Config::default());
        assert!(c.schedules.is_empty());
        assert!(c.active.is_empty());
        assert!(c.registry.take_executed().0.is_empty());
    }

    #[test]
    fn next_is_busy_during_decode_and_replaces_an_existing_fade_without_third_image() {
        let mut c = controller();
        start_slide(&mut c);
        c.command(7, NEXT, "{}");
        c.command(8, NEXT, "{}");
        assert_eq!(c.engine.status().requested, 2);
        assert!(matches!(c.owed.get(&2), Some(Owed::Next(7))));
        c.decoded(2, "/w/b.png".into(), decoded_ok());
        assert_eq!(c.resident_bytes(), 8);
        c.command(9, NEXT, "{}");
        assert!(
            c.previous.is_empty(),
            "the previous fade is released before another decode"
        );
        c.decoded(3, "/w/a.png".into(), decoded_ok());
        assert_eq!(
            c.resident_bytes(),
            8,
            "only the current and outgoing image remain"
        );
    }

    #[test]
    fn fade_cleanup_releases_old_pixels_and_wakes_even_without_a_surface() {
        let mut c = controller();
        start_slide(&mut c);
        c.command(7, NEXT, "{}");
        c.decoded(2, "/w/b.png".into(), decoded_ok());
        c.fade_started = Some(Instant::now() - FADE);
        c.tick();
        assert!(c.previous.is_empty());
        assert!(lock(&c.shared).view.previous.is_empty());
        assert_eq!(c.resident_bytes(), 4);
        assert_eq!(c.state()["slideshows"][0]["fading"], false);
    }

    #[test]
    fn timers_advance_only_due_outputs_and_do_not_record_bus_runs() {
        let mut c = controller();
        let mut config = slide_config();
        config.outputs.insert(
            "DP-1".into(),
            Wallpaper::Slideshow {
                source: SlideSource::Images(vec!["/w/c.png".into(), "/w/d.png".into()]),
                interval_s: 600,
                shuffle: false,
                fallback: None,
                fit: wallpaper::fit::Fit::Fill,
            },
        );
        c.stage(config, Owed::Startup);
        c.prepared(
            1,
            None,
            Ok((
                Schedule::new(
                    vec!["/w/a.png".into(), "/w/b.png".into()],
                    5,
                    false,
                    1,
                    Duration::ZERO,
                )
                .unwrap(),
                decoded_ok().unwrap(),
            )),
        );
        c.prepared(
            1,
            Some("DP-1".into()),
            Ok((
                Schedule::new(
                    vec!["/w/c.png".into(), "/w/d.png".into()],
                    600,
                    false,
                    1,
                    Duration::ZERO,
                )
                .unwrap(),
                decoded_ok().unwrap(),
            )),
        );
        c.started = Instant::now() - Duration::from_secs(6);
        c.tick();
        assert_eq!(c.engine.pending(), Some(2));
        assert_eq!(c.advancing, BTreeSet::from([None]));
        c.tick();
        assert_eq!(
            c.engine.status().requested,
            2,
            "no polling or repeat launch while decoding"
        );
        c.decoded(2, "/w/b.png".into(), decoded_ok());
        assert_eq!(
            c.schedules[&Some("DP-1".into())].current(),
            Path::new("/w/c.png")
        );
        assert_eq!(c.schedules[&None].current(), Path::new("/w/b.png"));
        assert!(c.registry.take_executed().0.is_empty());
    }

    #[test]
    fn a_stale_folder_completion_never_applies_after_a_new_set() {
        let mut c = controller();
        c.stage(slide_config(), Owed::Startup);
        c.stage(Config::default(), Owed::Startup);
        c.prepared(
            1,
            None,
            Ok((
                Schedule::new(vec!["/w/a.png".into()], 5, false, 1, Duration::ZERO).unwrap(),
                decoded_ok().unwrap(),
            )),
        );
        assert_eq!(c.engine.status().active, 2);
        assert!(c.schedules.is_empty());
        assert!(c.active.is_empty());
    }

    #[test]
    fn retained_view_snapshots_still_count_towards_the_cpu_budget() {
        let mut c = controller();
        let first = ["/w/1.png", "/w/2.png", "/w/3.png", "/w/4.png"];
        c.stage(images(&first), Owed::Startup);
        for path in first {
            c.decoded(1, path.into(), full_size());
        }
        let old_view = lock(&c.shared).view.clone();
        let second = ["/w/5.png", "/w/6.png", "/w/7.png", "/w/8.png"];
        c.stage(images(&second), Owed::Startup);
        for path in second {
            c.decoded(2, path.into(), full_size());
        }
        assert_eq!(c.resident_bytes(), 8 * 3840 * 3840 * 4);
        c.stage(images(&["/w/9.png", "/w/10.png"]), Owed::Startup);
        c.decoded(3, "/w/9.png".into(), full_size());
        c.decoded(3, "/w/10.png".into(), full_size());
        assert_eq!(
            c.engine.status().active,
            2,
            "the retained view prevents an over-budget commit"
        );
        assert!(c.engine.status().error.is_some());
        drop(old_view);
        assert_eq!(c.resident_bytes(), 4 * 3840 * 3840 * 4);
    }
}
