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
        self.requested += 1;
        let revision = self.requested;
        let superseded = self.pending.take().map(|p| p.revision);
        let jobs: Vec<PathBuf> = config
            .images()
            .into_iter()
            .filter(|path| !resident(path))
            .map(Path::to_path_buf)
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
}
