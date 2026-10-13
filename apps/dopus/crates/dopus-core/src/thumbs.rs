// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! dopus's side of thumbnails: one preview per pane, made by the shared
//! [`thumbs`](::thumbs) library. Each pane is a lane of the pool (a newer
//! selection replaces the request still waiting), its listing generation is
//! the request's ticket, and replies come back as
//! [`crate::events::CoreEvent::ThumbnailArrived`].

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, mpsc};

pub use ::thumbs::{Outcome, Size, cache, png};

use crate::events::CoreEvent;
use crate::model::PaneId;

/// The name dopus writes the shared cache as (its `fail/` directory and the
/// `Software` key).
pub const APP: &str = "dopus";

/// The size the Properties preview asks for: crisp at the panel's width on a
/// 2x output.
pub const PREVIEW_SIZE: Size = Size::XLarge;

/// A pane's preview of its selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preview {
    /// Being made.
    Pending,
    /// The thumbnail in the cache.
    Ready(PathBuf),
    /// None to show (failed or skipped): the MIME icon stays.
    None,
}

/// One pane's previews: what is being made, and the recent results by
/// `(generation, path)` (a late reply for an earlier selection never evicts
/// the current one).
#[derive(Default)]
pub(crate) struct Slot {
    pub in_flight: Option<(u64, PathBuf)>,
    pub results: Vec<(u64, PathBuf, Outcome)>,
}

/// Results kept per pane: enough to step back and forth over a selection
/// without asking again.
pub(crate) const KEPT: usize = 16;

impl Slot {
    pub(crate) fn result(&self, generation: u64, path: &PathBuf) -> Option<&Outcome> {
        self.results
            .iter()
            .find(|(g, p, _)| *g == generation && p == path)
            .map(|(_, _, outcome)| outcome)
    }

    /// Keep `outcome`, dropping results of other listings and the oldest
    /// past [`KEPT`].
    pub(crate) fn keep(&mut self, generation: u64, path: PathBuf, outcome: Outcome) {
        self.results
            .retain(|(g, p, _)| *g == generation && *p != path);
        self.results.push((generation, path, outcome));
        if self.results.len() > KEPT {
            self.results.remove(0);
        }
    }
}

/// The thumbnail pool, lane per pane, ticketed by the panes' listing
/// generations (the worker handle's mirror).
#[derive(Clone)]
pub(crate) struct Pool {
    pool: ::thumbs::Pool<PaneId>,
    generations: [Arc<AtomicU64>; 2],
}

impl Pool {
    pub(crate) fn new(tx: mpsc::Sender<CoreEvent>, generations: [Arc<AtomicU64>; 2]) -> Pool {
        let pool = ::thumbs::Pool::new(APP, move |reply: ::thumbs::Reply<PaneId>| {
            tx.send(CoreEvent::ThumbnailArrived {
                pane: reply.lane,
                generation: reply.generation,
                path: reply.path,
                outcome: reply.outcome,
            })
            .is_ok()
        });
        Pool { pool, generations }
    }

    /// Ask for `path`'s thumbnail for `pane` at its listing `generation`,
    /// replacing that pane's request still waiting (the selection moved on).
    pub(crate) fn submit(&self, pane: PaneId, generation: u64, path: PathBuf, size: Size) {
        self.pool.submit(::thumbs::Request {
            lane: pane,
            ticket: ::thumbs::Ticket::new(self.generations[pane.index()].clone(), generation),
            path,
            size,
        });
    }

    /// Thumbnail files on network and FUSE filesystems too (the
    /// `remote_previews` setting).
    pub(crate) fn set_remote(&self, remote: bool) {
        self.pool.set_remote(remote);
    }

    #[cfg(test)]
    pub(crate) fn remote(&self) -> bool {
        self.pool.remote()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_are_kept_per_selection() {
        let mut slot = Slot::default();
        slot.keep(1, PathBuf::from("b"), Outcome::Skipped("b"));
        slot.keep(1, PathBuf::from("a"), Outcome::Skipped("a"));
        assert_eq!(
            slot.result(1, &PathBuf::from("b")),
            Some(&Outcome::Skipped("b"))
        );
        slot.keep(2, PathBuf::from("c"), Outcome::Skipped("c"));
        assert!(
            slot.result(1, &PathBuf::from("a")).is_none(),
            "older listing dropped"
        );
        for i in 0..(KEPT + 4) {
            slot.keep(2, PathBuf::from(i.to_string()), Outcome::Skipped("x"));
        }
        assert_eq!(slot.results.len(), KEPT);
    }
}
