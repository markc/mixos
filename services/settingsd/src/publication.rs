// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Change-only publication of the 1.0.0 topics (key space v2 §5.1).
//!
//! The [`Board`] holds each topic's last content and the revision that last
//! changed it (`changed_at`). Staging a revision marks pending only the
//! topics whose content moved; a broker reconnect marks every topic
//! pending, because retained state is the broker's and a restarted broker
//! has none. The service publishes what is pending and marks each one
//! published; a failure leaves it pending for the bounded retry.
//!
//! Topics are named by a short key: the prefix for a `keys` topic, and
//! `look` and `snapshot` for the other two. The same keys name `changed_at`
//! in a mirror push, so a mirror publishes byte-identical bodies.
use serde_json::Value;
use settings::Revision;
use std::collections::BTreeMap;

pub const LOOK: &str = "look";
pub const SNAPSHOT: &str = "snapshot";

#[derive(Clone, Debug)]
struct Entry {
    content: Value,
    changed_at: Revision,
    pending: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Board {
    entries: BTreeMap<String, Entry>,
}

impl Board {
    /// Record every topic's content at `revision`. A topic whose content
    /// moved (or is new) becomes pending with `changed_at` = `revision`,
    /// unless `changed_at` names the revision that changed it (a mirror
    /// adopting its authority's). Returns the keys that moved.
    pub fn stage(
        &mut self,
        revision: Revision,
        contents: BTreeMap<String, Value>,
        changed_at: Option<&BTreeMap<String, Revision>>,
    ) -> Vec<String> {
        let mut moved = Vec::new();
        self.entries.retain(|key, _| contents.contains_key(key));
        for (key, content) in contents {
            let given = changed_at.and_then(|m| m.get(&key)).copied();
            match self.entries.get_mut(&key) {
                Some(entry) if entry.content == content => {
                    // Same content: a mirror still adopts a later stamp, so
                    // its bodies stay identical to the authority's.
                    if let Some(at) = given
                        && at != entry.changed_at
                    {
                        entry.changed_at = at;
                        entry.pending = true;
                        moved.push(key);
                    }
                }
                _ => {
                    self.entries.insert(
                        key.clone(),
                        Entry {
                            content,
                            changed_at: given.unwrap_or(revision),
                            pending: true,
                        },
                    );
                    moved.push(key);
                }
            }
        }
        moved
    }

    /// Every topic pending again: a (re)connected broker has no retained
    /// state of ours.
    pub fn republish_all(&mut self) {
        for entry in self.entries.values_mut() {
            entry.pending = true;
        }
    }

    pub fn pending(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|(_, e)| e.pending)
            .map(|(k, _)| k.clone())
            .collect()
    }

    pub fn is_current(&self) -> bool {
        self.entries.values().all(|e| !e.pending)
    }

    pub fn published(&mut self, key: &str) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.pending = false;
        }
    }

    pub fn content(&self, key: &str) -> Option<&Value> {
        self.entries.get(key).map(|e| &e.content)
    }

    pub fn changed_at(&self) -> BTreeMap<String, Revision> {
        self.entries
            .iter()
            .map(|(k, e)| (k.clone(), e.changed_at))
            .collect()
    }

    pub fn changed_at_of(&self, key: &str) -> Option<Revision> {
        self.entries.get(key).map(|e| e.changed_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contents(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    #[test]
    fn only_moved_topics_are_pending_and_reconnect_makes_all_pending() {
        let mut board = Board::default();
        let first = board.stage(
            Revision(1),
            contents(&[("time", json!(1)), ("appearance", json!(1))]),
            None,
        );
        assert_eq!(first, ["appearance", "time"]);
        for key in first {
            board.published(&key);
        }
        assert!(board.is_current());
        let moved = board.stage(
            Revision(2),
            contents(&[("time", json!(2)), ("appearance", json!(1))]),
            None,
        );
        assert_eq!(moved, ["time"]);
        assert_eq!(board.pending(), ["time"]);
        assert_eq!(board.changed_at_of("time"), Some(Revision(2)));
        assert_eq!(board.changed_at_of("appearance"), Some(Revision(1)));
        board.published("time");
        board.republish_all();
        assert_eq!(board.pending(), ["appearance", "time"]);
    }

    #[test]
    fn a_mirror_adopts_its_authoritys_stamps() {
        let mut board = Board::default();
        let stamps = BTreeMap::from([("time".to_owned(), Revision(4))]);
        board.stage(Revision(9), contents(&[("time", json!(1))]), Some(&stamps));
        assert_eq!(board.changed_at_of("time"), Some(Revision(4)));
        board.published("time");
        // Same content, a later stamp (the mirror started from a snapshot
        // that predates the stamp): republished with the authority's.
        let stamps = BTreeMap::from([("time".to_owned(), Revision(5))]);
        let moved = board.stage(Revision(9), contents(&[("time", json!(1))]), Some(&stamps));
        assert_eq!(moved, ["time"]);
        assert_eq!(board.changed_at_of("time"), Some(Revision(5)));
    }
}
