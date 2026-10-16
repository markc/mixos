// SPDX-License-Identifier: MIT OR Apache-2.0

use std::time::{SystemTime, UNIX_EPOCH};

use comp_model::observation::ObservationRecord;
use comp_model::snapshot::SessionSnapshot;

/// The initial session state and each active/paused edge, with one clock
/// shared by the query and topic. Each record carries its own observation time.
pub(super) struct Session {
    snapshot: SessionSnapshot,
    announced: bool,
}

impl Session {
    pub(super) fn new(vt: Option<&str>, started_ms: u64) -> Self {
        Self {
            snapshot: SessionSnapshot {
                active: true,
                vt: vt.and_then(|value| value.parse::<u32>().ok()),
                since_ms: started_ms,
            },
            announced: false,
        }
    }

    pub(super) fn observe(&mut self, active: bool, now_ms: u64) -> Option<ObservationRecord> {
        if self.announced && self.snapshot.active == active {
            return None;
        }
        // The first record may be a later edge (a compd started on a VT in
        // the background is first serviced when the VT activates): its time
        // is that edge's, never the process start.
        self.snapshot.since_ms = now_ms;
        self.snapshot.active = active;
        self.announced = true;
        Some(ObservationRecord::SessionChanged {
            active,
            vt: self.snapshot.vt,
            since_ms: self.snapshot.since_ms,
            event_seq: 0,
        })
    }

    pub(super) fn snapshot(&self) -> SessionSnapshot {
        self.snapshot
    }
}

pub(super) fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_and_both_edges_publish_once_and_keep_the_query_in_step() {
        for initially_active in [true, false] {
            let mut session = Session::new(Some("3"), 100);
            for (active, now_ms, since_ms) in [
                (initially_active, 110, 110),
                (!initially_active, 200, 200),
                (initially_active, 300, 300),
            ] {
                assert_eq!(
                    session.observe(active, now_ms),
                    Some(ObservationRecord::SessionChanged {
                        active,
                        vt: Some(3),
                        since_ms,
                        event_seq: 0,
                    })
                );
                assert_eq!(
                    session.snapshot(),
                    SessionSnapshot {
                        active,
                        vt: Some(3),
                        since_ms,
                    }
                );
                assert_eq!(session.observe(active, now_ms + 1), None);
                assert_eq!(session.snapshot().since_ms, since_ms);
            }
        }
    }

    #[test]
    fn vt_is_only_the_supplied_u32() {
        for (value, expected) in [
            (None, None),
            (Some(""), None),
            (Some("tty3"), None),
            (Some("-1"), None),
            (Some("4294967296"), None),
            (Some("3"), Some(3)),
            (Some("4294967295"), Some(u32::MAX)),
        ] {
            let mut session = Session::new(value, 100);
            let _ = session.observe(true, 110);
            assert_eq!(session.snapshot().vt, expected, "{value:?}");
        }
    }
}
