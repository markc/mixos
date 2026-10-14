// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The revisions of the backdrop. Every request is numbered (`requested`). A
//! request whose images are all decoded applies at once, and the revision it
//! carries becomes `active`. One still waiting on decodes is pending; a newer
//! request supersedes it, so only the latest request can apply. A request
//! whose decode fails is dropped and the last applied one stays on screen,
//! with the reason in `error`.
//!
//! The engine never decodes and never draws: the controller runs the jobs it
//! names and reports each result back. The applied configuration is changed
//! only by [`Engine::apply`], which the Bus command runs.

use crate::schema::{Config, Document};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A slideshow's pure schedule. Times are durations on the controller's
/// monotonic clock. Clone before advancing: a refused decode leaves the active
/// selection unchanged. A single image has no automatic wake.
#[derive(Debug, Clone)]
pub struct Schedule {
    pub files: Vec<PathBuf>,
    pub current_index: usize,
    pub interval_s: u64,
    pub shuffle: bool,
    order: Vec<usize>,
    position: usize,
    random: u64,
    next_due: Option<Duration>,
}

impl Schedule {
    pub fn new(files: Vec<PathBuf>, interval_s: u64, shuffle: bool, seed: u64, now: Duration) -> Result<Self, String> {
        if files.is_empty() || files.len() > crate::schema::MAX_SLIDES || interval_s < 5 {
            return Err("slideshow needs a bounded non-empty source and interval of at least 5 seconds".into());
        }
        let mut unique = BTreeSet::new();
        let files: Vec<_> = files.into_iter().filter(|p| unique.insert(p.clone())).collect();
        let mut schedule = Self { order: (0..files.len()).collect(), files,
            current_index: 0, interval_s, shuffle, position: 0,
            random: if seed == 0 { 0x9e3779b97f4a7c15 } else { seed }, next_due: None };
        if shuffle { schedule.permute(None); }
        schedule.current_index = schedule.order[0];
        schedule.defer(now);
        Ok(schedule)
    }

    fn random(&mut self) -> u64 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        self.random
    }

    fn permute(&mut self, last: Option<usize>) {
        for i in (1..self.order.len()).rev() {
            let j = (self.random() % (i as u64 + 1)) as usize;
            self.order.swap(i, j);
        }
        if self.order.len() > 1 && last == Some(self.order[0]) { self.order.swap(0, 1); }
    }

    pub fn current(&self) -> &Path { &self.files[self.current_index] }
    pub fn deadline(&self) -> Option<Duration> { self.next_due }
    pub fn due(&self, now: Duration) -> bool { self.next_due.is_some_and(|t| t <= now) }

    pub fn advance(&mut self, now: Duration) {
        self.position += 1;
        if self.position == self.order.len() {
            self.position = 0;
            if self.shuffle { self.permute(Some(self.current_index)); }
        }
        self.current_index = self.order[self.position];
        self.defer(now);
    }

    /// Schedule from completion or failure, rather than catching up old ticks.
    pub fn defer(&mut self, now: Duration) {
        self.next_due = if self.files.len() > 1 { now.checked_add(Duration::from_secs(self.interval_s)) } else { None };
    }
}

#[derive(Debug)]
struct Pending {
    revision: u64,
    config: Config,
    /// The images not decoded yet.
    waiting: BTreeSet<PathBuf>,
}

/// What a request did: its revision, the images it needs decoded, and the
/// request it superseded, if one was still waiting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub revision: u64,
    pub jobs: Vec<PathBuf>,
    pub superseded: Option<u64>,
}

/// What a decode result did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decode {
    /// Not the request waiting now: discarded.
    Superseded,
    /// The request still waits on other images.
    Waiting,
    /// Every image the request needs is decoded: it can apply.
    Ready,
    /// The request is dropped; the message names the image and why.
    Failed(String),
}

/// The state a query reports.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub requested: u64,
    pub active: u64,
    pub pending: Option<u64>,
    pub error: Option<String>,
    pub config: Document,
}

#[derive(Debug)]
pub struct Engine {
    requested: u64,
    active: u64,
    applied: Config,
    pending: Option<Pending>,
    error: Option<String>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// No request yet: the session theme, revision 0.
    pub fn new() -> Self {
        Self {
            requested: 0,
            active: 0,
            applied: Config::default(),
            pending: None,
            error: None,
        }
    }

    /// The configuration on screen.
    pub fn applied(&self) -> &Config {
        &self.applied
    }

    /// Number a request for `config`. Images for which `resident` is false
    /// must be decoded first; a request with none applies at once (see
    /// [`Engine::ready`]).
    pub fn stage(&mut self, config: Config, resident: impl Fn(&Path) -> bool) -> Staged {
        let paths = config.images().into_iter().map(Path::to_path_buf).collect();
        self.stage_images(config, paths, resident)
    }

    /// Stage the worker-selected paths, or relative discovery markers for
    /// slideshow sources. The public configuration keeps its source declarations.
    pub fn stage_images(&mut self, config: Config, paths: Vec<PathBuf>, resident: impl Fn(&Path) -> bool) -> Staged {
        self.requested += 1;
        let revision = self.requested;
        let superseded = self.pending.take().map(|p| p.revision);
        let jobs: Vec<PathBuf> = paths.into_iter().collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|path| !resident(path))
            .collect();
        let waiting = jobs.iter().cloned().collect();
        self.pending = Some(Pending {
            revision,
            config,
            waiting,
        });
        Staged {
            revision,
            jobs,
            superseded,
        }
    }

    /// The image `path` of request `revision` finished decoding, or failed.
    pub fn decoded(&mut self, revision: u64, path: &Path, result: Result<(), String>) -> Decode {
        let Some(pending) = self.pending.as_mut().filter(|p| p.revision == revision) else {
            return Decode::Superseded;
        };
        match result {
            Err(reason) => {
                let message = format!("{}: {reason}", path.display());
                self.pending = None;
                self.error = Some(message.clone());
                Decode::Failed(message)
            }
            Ok(()) => {
                pending.waiting.remove(path);
                if pending.waiting.is_empty() {
                    Decode::Ready
                } else {
                    Decode::Waiting
                }
            }
        }
    }

    /// The waiting request, if every image it needs is decoded.
    pub fn ready(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| p.waiting.is_empty())
    }

    /// The revision waiting to apply or to finish decoding.
    pub fn pending(&self) -> Option<u64> {
        self.pending.as_ref().map(|p| p.revision)
    }

    /// The configuration waiting to apply or to finish decoding.
    pub fn pending_config(&self) -> Option<&Config> {
        self.pending.as_ref().map(|p| &p.config)
    }

    /// Apply the request that is [`ready`](Engine::ready). Returns its
    /// revision, or `None` when nothing is ready.
    pub fn apply(&mut self) -> Option<u64> {
        if !self.ready() {
            return None;
        }
        let pending = self.pending.take()?;
        self.applied = pending.config;
        self.active = pending.revision;
        self.error = None;
        Some(pending.revision)
    }

    /// A ready request could not be stored: drop it, keep the applied one.
    pub fn refuse(&mut self, revision: u64, reason: String) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.revision == revision)
        {
            self.pending = None;
            self.error = Some(reason);
            true
        } else {
            false
        }
    }

    /// The state a query reports.
    pub fn status(&self) -> Status {
        Status {
            requested: self.requested,
            active: self.active,
            pending: self.pending(),
            error: self.error.clone(),
            config: self.applied.document(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colour::Rgb;
    use crate::schema::Wallpaper;
    use std::collections::BTreeMap;

    fn image(path: &str) -> Config {
        Config {
            default: Wallpaper::Image {
                path: PathBuf::from(path),
                fallback: None,
                fit: crate::fit::Fit::Fill,
            },
            outputs: BTreeMap::new(),
        }
    }

    fn colour() -> Config {
        Config {
            default: Wallpaper::Colour(Rgb([1, 2, 3])),
            outputs: BTreeMap::new(),
        }
    }

    #[test]
    fn a_request_without_images_applies_at_once() {
        let mut engine = Engine::new();
        let staged = engine.stage(colour(), |_| false);
        assert_eq!(staged.revision, 1);
        assert!(staged.jobs.is_empty());
        assert!(engine.ready());
        assert_eq!(engine.apply(), Some(1));
        assert_eq!(engine.applied(), &colour());
        let status = engine.status();
        assert_eq!((status.requested, status.active), (1, 1));
        assert_eq!(status.pending, None);
    }

    #[test]
    fn an_image_applies_only_once_it_is_decoded() {
        let mut engine = Engine::new();
        let staged = engine.stage(image("/a.png"), |_| false);
        assert_eq!(staged.jobs, vec![PathBuf::from("/a.png")]);
        assert!(!engine.ready());
        assert_eq!(engine.apply(), None);
        assert_eq!(engine.applied(), &Config::default());
        assert_eq!(
            engine.decoded(1, Path::new("/a.png"), Ok(())),
            Decode::Ready
        );
        assert_eq!(engine.apply(), Some(1));
        assert_eq!(engine.applied(), &image("/a.png"));
    }

    #[test]
    fn a_newer_request_supersedes_one_still_decoding() {
        let mut engine = Engine::new();
        engine.stage(image("/a.png"), |_| false);
        let second = engine.stage(image("/b.png"), |_| false);
        assert_eq!(second.superseded, Some(1));
        assert_eq!(
            engine.decoded(1, Path::new("/a.png"), Ok(())),
            Decode::Superseded,
            "a superseded completion never touches the request that replaced it"
        );
        assert_eq!(engine.pending(), Some(2));
        assert_eq!(engine.applied(), &Config::default());
        assert_eq!(
            engine.decoded(2, Path::new("/b.png"), Ok(())),
            Decode::Ready
        );
        assert_eq!(engine.apply(), Some(2));
    }

    #[test]
    fn a_refused_image_keeps_the_last_good_backdrop() {
        let mut engine = Engine::new();
        engine.stage(colour(), |_| false);
        engine.apply();
        engine.stage(image("/missing.png"), |_| false);
        let outcome = engine.decoded(2, Path::new("/missing.png"), Err("no such file".into()));
        assert!(matches!(&outcome, Decode::Failed(m) if m.contains("no such file")));
        let status = engine.status();
        assert_eq!((status.requested, status.active), (2, 1));
        assert!(
            status
                .error
                .as_deref()
                .is_some_and(|e| e.contains("no such file"))
        );
        assert_eq!(status.pending, None);
        assert_eq!(engine.applied(), &colour());
        assert_eq!(engine.apply(), None, "nothing waits to apply");
    }

    #[test]
    fn a_resident_image_needs_no_new_decode() {
        let mut engine = Engine::new();
        let staged = engine.stage(image("/a.png"), |p| p == Path::new("/a.png"));
        assert!(staged.jobs.is_empty());
        assert!(engine.ready());
    }

    #[test]
    fn a_refused_store_drops_only_the_revision_named() {
        let mut engine = Engine::new();
        engine.stage(colour(), |_| false);
        assert!(!engine.refuse(9, "not this one".into()));
        assert!(engine.refuse(1, "disk full".into()));
        assert_eq!(engine.apply(), None);
        assert!(engine.status().error.as_deref() == Some("disk full"));
    }

    fn schedule(shuffle: bool) -> Schedule {
        Schedule::new(["/a.png", "/b.png", "/c.png"].map(PathBuf::from).to_vec(), 5, shuffle, 42, Duration::ZERO).unwrap()
    }

    #[test]
    fn sequential_schedule_wraps_and_uses_completion_time() {
        let mut s = schedule(false);
        assert_eq!(s.current(), Path::new("/a.png"));
        assert!(!s.due(Duration::from_secs(4)));
        assert!(s.due(Duration::from_secs(5)));
        s.advance(Duration::from_secs(7));
        assert_eq!(s.current(), Path::new("/b.png"));
        assert_eq!(s.deadline(), Some(Duration::from_secs(12)));
        s.advance(Duration::from_secs(12));
        assert_eq!(s.current(), Path::new("/c.png"));
        s.advance(Duration::from_secs(17));
        assert_eq!(s.current(), Path::new("/a.png"));
    }

    #[test]
    fn shuffle_visits_each_image_once_per_cycle_without_boundary_repeats() {
        let mut s = schedule(true);
        let mut previous = None;
        for cycle in 0..20 {
            let mut seen = BTreeSet::new();
            for step in 0..3 {
                let path = s.current().to_path_buf();
                assert_ne!(previous.as_ref(), Some(&path));
                assert!(seen.insert(path.clone()));
                previous = Some(path);
                s.advance(Duration::from_secs((cycle * 3 + step + 1) * 5));
            }
            assert_eq!(seen.len(), 3);
        }
    }

    #[test]
    fn a_failed_proposal_preserves_selection_and_defers_retry() {
        let mut active = schedule(false);
        let mut proposal = active.clone();
        proposal.advance(Duration::from_secs(5));
        assert_ne!(proposal.current(), active.current());
        active.defer(Duration::from_secs(8));
        assert_eq!(active.current(), Path::new("/a.png"));
        assert_eq!(active.deadline(), Some(Duration::from_secs(13)));
    }

    #[test]
    fn one_image_has_no_timer_but_can_be_refreshed_manually() {
        let mut s = Schedule::new(vec!["/a.png".into(), "/a.png".into()], 5, true, 0, Duration::ZERO).unwrap();
        assert_eq!(s.deadline(), None);
        assert!(!s.due(Duration::MAX));
        s.advance(Duration::from_secs(100));
        assert_eq!(s.current(), Path::new("/a.png"));
        assert_eq!(s.deadline(), None);
    }

    #[test]
    fn schedule_refuses_invalid_inputs_and_bounds_large_deadlines() {
        assert!(Schedule::new(vec![], 5, false, 1, Duration::ZERO).is_err());
        assert!(Schedule::new(vec!["/a.png".into()], 4, false, 1, Duration::ZERO).is_err());
        let mut s = schedule(false);
        s.defer(Duration::MAX);
        assert_eq!(s.deadline(), None);
    }
}
