// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The mirror role's Bus half (key space v2 §5.4, ruling F1).
//!
//! Topics are node-local, so one authority per machine pushes each revision
//! to every attached mirror by command (`settings.mirror.revision` to
//! `settingsd.<node>.bus`), and each mirror republishes the same topics on
//! its own node. [`Attached`] is the authority's table: who is attached,
//! what each last acknowledged, and the bounded retry per mirror. A mirror
//! that keeps failing is dropped and re-attaches itself on its own broker
//! reconnect event. No heartbeat: an attached mirror that hears nothing is
//! current.
//!
//! The attached nodes are also kept beside the store (`mirrors.json`, a
//! plain list, not sealed), so an authority that restarts pushes to them
//! at once instead of leaving them stale until their own next reconnect.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use settings::{Binding, Revision};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::Instant;

/// Consecutive failed pushes before a mirror is dropped. With the shared
/// backoff (250 ms doubling to 30 s) that is about a minute of trying.
pub const MAX_PUSH_FAILURES: u8 = 8;
/// One push waits this long for its acknowledgement.
pub const PUSH_TIMEOUT: Duration = Duration::from_secs(5);
/// A forwarded write waits this long for the authority's receipt.
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(15);

/// The bounded backoff shared with topic publication: 250 ms doubling,
/// at most 30 s.
pub fn backoff(failures: u8) -> Duration {
    let failures = failures.max(1);
    Duration::from_millis((250 * (1u64 << (failures - 1).min(7))).min(30_000))
}

/// The service name of settingsd on `node`.
pub fn address(node: &str) -> String {
    format!("settingsd.{node}.bus")
}

/// `settings.mirror.attach`: mirror → authority.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachRequest {
    pub binding: Binding,
    /// The mirror's node, where pushes go.
    pub node: String,
    /// The revision the mirror holds; behind the authority's, it is pushed
    /// at once.
    pub revision: Revision,
}

/// `settings.mirror.revision`: authority → mirror.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionPush {
    pub binding: Binding,
    /// The authority's node.
    pub authority: String,
    /// The authority's record as the mirror caches it, sealed under the
    /// authority's node: its values, revisions and pinned source, so the
    /// mirror's resolve, snapshot and topics are the authority's. Omitted
    /// on a `stopping` notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<crate::mirror::Cache>,
    /// The prefixes whose values changed since the mirror's revision; all
    /// of them on a first or catch-up push.
    pub changed: Vec<String>,
    /// The revision that last changed each topic, so the mirror's bodies
    /// equal the authority's.
    pub changed_at: BTreeMap<String, Revision>,
    /// The authority is stopping: the mirror marks itself stale until the
    /// next push. Sent once on a clean shutdown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopping: bool,
}

#[derive(Clone, Debug)]
struct Mirror {
    /// The revision the mirror last acknowledged.
    acked: Option<Revision>,
    /// A push is in flight.
    busy: bool,
    failures: u8,
    retry_at: Option<Instant>,
    /// Known only from `mirrors.json`: neither attached nor acknowledged in
    /// this run. The file is a hint, not authority, so one failed push
    /// drops it.
    hinted: bool,
}

impl Mirror {
    fn new(acked: Option<Revision>) -> Self {
        Self {
            acked,
            busy: false,
            failures: 0,
            retry_at: None,
            hinted: false,
        }
    }
}

/// The authority's attached mirrors.
#[derive(Debug, Default)]
pub struct Attached {
    mirrors: BTreeMap<String, Mirror>,
    file: Option<PathBuf>,
}

impl Attached {
    /// The table kept in `root`, with the nodes a previous run had attached
    /// (each to be pushed at once). An unreadable file is an empty table.
    pub fn load(root: &Path) -> Self {
        let file = root.join("mirrors.json");
        let nodes: BTreeSet<String> = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            mirrors: nodes
                .into_iter()
                .filter(|n| valid_node(n))
                .map(|n| {
                    let mut mirror = Mirror::new(None);
                    mirror.hinted = true;
                    (n, mirror)
                })
                .collect(),
            file: Some(file),
        }
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        let nodes: Vec<&String> = self.mirrors.keys().collect();
        let tmp = file.with_extension("json.tmp");
        let result = serde_json::to_vec(&nodes)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&tmp, bytes))
            .and_then(|()| std::fs::rename(&tmp, file));
        if let Err(error) = result {
            tracing::warn!(%error, "mirror list not saved; a restart pushes only to re-attached mirrors");
        }
    }

    /// Attach (or re-attach) `node` holding `revision`.
    pub fn attach(&mut self, node: &str, revision: Revision) {
        let fresh = !self.mirrors.contains_key(node);
        let entry = self
            .mirrors
            .entry(node.to_owned())
            .or_insert_with(|| Mirror::new(None));
        entry.acked = Some(revision);
        entry.failures = 0;
        entry.retry_at = None;
        entry.hinted = false;
        if fresh {
            self.save();
        }
    }

    pub fn nodes(&self) -> Vec<String> {
        self.mirrors.keys().cloned().collect()
    }

    /// Every mirror needs `current` again (an authority (re)connect).
    pub fn forget_acks(&mut self) {
        for mirror in self.mirrors.values_mut() {
            mirror.acked = None;
            mirror.failures = 0;
            mirror.retry_at = None;
        }
    }

    /// The mirrors to push `current` to now: behind it, idle, and not
    /// waiting out a backoff. Each returned is marked busy.
    pub fn due(&mut self, current: Revision, now: Instant) -> Vec<(String, Option<Revision>)> {
        let mut out = Vec::new();
        for (node, mirror) in &mut self.mirrors {
            if mirror.busy || mirror.acked == Some(current) {
                continue;
            }
            if mirror.retry_at.is_some_and(|at| at > now) {
                continue;
            }
            mirror.busy = true;
            mirror.retry_at = None;
            out.push((node.clone(), mirror.acked));
        }
        out
    }

    /// The outcome of one push. A mirror failing [`MAX_PUSH_FAILURES`]
    /// times in a row is dropped, and one known only from `mirrors.json` on
    /// its first failure; returns true when it was.
    pub fn finished(&mut self, node: &str, revision: Revision, ok: bool, now: Instant) -> bool {
        let Some(mirror) = self.mirrors.get_mut(node) else {
            return false;
        };
        mirror.busy = false;
        if ok {
            mirror.acked = Some(revision);
            mirror.failures = 0;
            mirror.retry_at = None;
            mirror.hinted = false;
            return false;
        }
        mirror.failures = mirror.failures.saturating_add(1);
        if mirror.hinted || mirror.failures >= MAX_PUSH_FAILURES {
            self.mirrors.remove(node);
            self.save();
            return true;
        }
        mirror.retry_at = Some(now + backoff(mirror.failures));
        false
    }

    /// When the next backoff ends, if any mirror is waiting one out.
    pub fn next_retry(&self) -> Option<Instant> {
        self.mirrors.values().filter_map(|m| m.retry_at).min()
    }

    pub fn status(&self) -> Value {
        self.mirrors
            .iter()
            .map(|(node, m)| {
                (
                    node.clone(),
                    json!({"revision": m.acked, "failures": m.failures}),
                )
            })
            .collect::<serde_json::Map<_, _>>()
            .into()
    }
}

/// A node name as the Bus addresses it.
pub fn valid_node(node: &str) -> bool {
    !node.is_empty()
        && node.len() <= 63
        && node
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether a forwarded reply came from the authority itself. Every settingsd
/// answer to a write is a JSON object with a `status`; a broker that cannot
/// reach the authority (no route, no such service, a dropped mesh link)
/// answers without one.
pub fn from_authority(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("status").cloned())
        .is_some_and(|s| s.is_string())
}

/// The refusal for a write a mirror cannot forward.
pub fn unreachable(node: &str) -> Value {
    json!({"status": "authority_unreachable", "node": node,
        "message": format!("The settings authority on {node} cannot be reached; writes resume when it is back")})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_behind_mirror_is_pushed_once_and_a_failing_one_backs_off_then_drops() {
        let mut table = Attached::default();
        let now = Instant::now();
        table.attach("beta", Revision(3));
        assert!(table.due(Revision(3), now).is_empty(), "current: no push");
        assert_eq!(
            table.due(Revision(4), now),
            [("beta".to_owned(), Some(Revision(3)))]
        );
        assert!(table.due(Revision(4), now).is_empty(), "one in flight");
        assert!(!table.finished("beta", Revision(4), false, now));
        assert!(table.due(Revision(4), now).is_empty(), "backing off");
        let at = table.next_retry().unwrap();
        assert_eq!(table.due(Revision(4), at).len(), 1);
        for _ in 1..MAX_PUSH_FAILURES - 1 {
            assert!(!table.finished("beta", Revision(4), false, now));
            let at = table.next_retry().unwrap();
            assert!(at <= now + Duration::from_secs(30));
            assert_eq!(table.due(Revision(4), at).len(), 1);
        }
        assert!(table.finished("beta", Revision(4), false, now), "dropped");
        assert!(table.nodes().is_empty());
        assert!(table.next_retry().is_none(), "no idle work");
    }

    #[test]
    fn success_acknowledges_and_reconnect_pushes_everyone_again() {
        let mut table = Attached::default();
        let now = Instant::now();
        table.attach("gamma", Revision(1));
        table.attach("beta", Revision(1));
        assert_eq!(table.due(Revision(2), now).len(), 2);
        table.finished("gamma", Revision(2), true, now);
        table.finished("beta", Revision(2), true, now);
        assert!(table.due(Revision(2), now).is_empty());
        table.forget_acks();
        assert_eq!(table.due(Revision(2), now).len(), 2);
    }

    #[test]
    fn a_stale_entry_in_the_file_costs_one_failed_push_then_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut table = Attached::load(dir.path());
        table.attach("gamma", Revision(2));
        table.attach("delta", Revision(2));
        // A restart: both are hints until they acknowledge.
        let mut again = Attached::load(dir.path());
        let now = Instant::now();
        assert_eq!(again.due(Revision(2), now).len(), 2);
        assert!(
            again.finished("gamma", Revision(2), false, now),
            "gone after one failure"
        );
        assert!(!again.finished("delta", Revision(2), true, now));
        assert_eq!(again.nodes(), ["delta"]);
        assert!(
            again.next_retry().is_none(),
            "no retry is left for the stale entry"
        );
        assert_eq!(
            Attached::load(dir.path()).nodes(),
            ["delta"],
            "and the file forgets it"
        );
        // Acknowledged once, the mirror has the normal bounded retry.
        assert_eq!(again.due(Revision(3), now).len(), 1);
        assert!(!again.finished("delta", Revision(3), false, now));
        assert_eq!(again.nodes(), ["delta"]);
    }

    #[test]
    fn the_table_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut table = Attached::load(dir.path());
        table.attach("delta", Revision(7));
        let again = Attached::load(dir.path());
        assert_eq!(again.nodes(), ["delta"]);
        let mut again = again;
        assert_eq!(
            again.due(Revision(7), Instant::now()),
            [("delta".to_owned(), None)],
            "pushed at once after a restart"
        );
    }

    #[test]
    fn only_a_settingsd_answer_is_relayed() {
        assert!(from_authority(r#"{"status":"conflict","revision":3}"#));
        assert!(!from_authority(""));
        assert!(!from_authority(r#"{"error_code":"UNKNOWN_COMMAND"}"#));
        assert!(!from_authority("Mesh bridge error: closed"));
    }
}
