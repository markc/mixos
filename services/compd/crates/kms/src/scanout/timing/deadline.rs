// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fixed-refresh late-latch arithmetic. All clocks and durations come from the caller.

use std::time::{Duration, Instant};

/// Two 50-frame buckets retain a maximum for 50–100 submitted frames. Old
/// outliers expire without allocating or walking a sample window each frame.
#[derive(Debug)]
pub struct LateLatch {
    current: Duration,
    previous: Duration,
    samples: u16,
    backoff: Duration,
    backoff_left: u16,
    candidate: Option<Target>,
    submitted: Option<Target>,
    following_flip: bool,
    pub budget_used: Duration,
    pub submitted_budget: Duration,
    pub armed: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub vblank: Instant,
    pub refresh: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub target: Option<Target>,
    pub budget: Duration,
    pub deadline: Option<Instant>,
}

impl Default for LateLatch {
    fn default() -> Self {
        Self {
            current: Duration::ZERO,
            // Conservative seed until the first measured frames replace it.
            previous: Duration::from_millis(2),
            samples: 0,
            backoff: Duration::ZERO,
            backoff_left: 0,
            candidate: None,
            submitted: None,
            following_flip: false,
            budget_used: Duration::ZERO,
            submitted_budget: Duration::ZERO,
            armed: None,
        }
    }
}

impl LateLatch {
    pub fn following_flip(&self) -> bool {
        self.following_flip
    }
    pub fn submitted_target(&self) -> Option<Target> {
        self.submitted
    }

    pub fn budget(&self, margin: Duration) -> Duration {
        self.current
            .max(self.previous)
            .saturating_add(margin)
            .saturating_add(self.backoff)
    }

    /// Only actual, trustworthy flip timestamps create a candidate. An idle
    /// completion parks the pipe; a later request cannot spend this old phase.
    /// Half an interval tolerates timestamp rounding, but detects a flip a full
    /// vblank late. Dispatch delay is absent from both sides of the comparison.
    pub fn flipped(
        &mut self,
        at: Option<Instant>,
        refresh: Duration,
        pending: bool,
    ) -> Option<bool> {
        let hit = self.submitted.take().and_then(|target| {
            at.filter(|_| target.refresh == refresh).map(|actual| {
                let hit = actual < target.vblank + refresh / 2;
                if !hit {
                    self.backoff = (self.backoff.saturating_mul(2))
                        .max(refresh / 4)
                        .min(refresh);
                    self.backoff_left = 100;
                }
                hit
            })
        });
        self.armed = None;
        self.following_flip = pending;
        self.candidate = at.filter(|_| pending && !refresh.is_zero()).and_then(|at| {
            at.checked_add(refresh)
                .map(|vblank| Target { vblank, refresh })
        });
        hit
    }

    pub fn plan(&self, now: Instant, margin: Duration) -> Plan {
        let budget = self.budget(margin);
        let deadline = self
            .candidate
            .and_then(|target| target.vblank.checked_sub(budget).filter(|at| *at > now));
        Plan {
            target: self.candidate,
            budget,
            deadline,
        }
    }

    /// Empty frames, unsafe modes, rebuilds and idle all discard the phase.
    pub fn park(&mut self) {
        self.candidate = None;
        self.following_flip = false;
        self.armed = None;
    }

    /// Called only once queue/commit has returned successfully. Includes scene
    /// preparation, GPU submission, any sync wait and the DRM commit itself.
    pub fn committed(&mut self, elapsed: Duration) {
        self.submitted = self.candidate.take();
        self.submitted_budget = self.budget_used;
        self.following_flip = false;
        self.armed = None;
        self.current = self.current.max(elapsed);
        self.samples += 1;
        if self.samples == 50 {
            self.previous = self.current;
            self.current = Duration::ZERO;
            self.samples = 0;
        }
        if self.backoff_left > 0 {
            self.backoff_left -= 1;
        } else {
            self.backoff /= 2;
            if self.backoff < Duration::from_micros(100) {
                self.backoff = Duration::ZERO;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PERIOD: Duration = Duration::from_millis(16);
    const MARGIN: Duration = Duration::from_micros(1500);

    #[test]
    fn prediction_uses_flip_not_dispatch_and_clamps_late_or_large_budgets() {
        let at = Instant::now();
        let mut latch = LateLatch::default();
        latch.flipped(Some(at), PERIOD, true);
        let plan = latch.plan(at + Duration::from_millis(4), MARGIN);
        assert_eq!(plan.target.unwrap().vblank, at + PERIOD);
        assert_eq!(
            plan.deadline,
            Some(at + PERIOD - Duration::from_micros(3500))
        );
        assert!(latch.plan(at + PERIOD, MARGIN).deadline.is_none());
        assert!(latch.plan(at, PERIOD).deadline.is_none());
    }

    #[test]
    fn maximum_expires_within_one_hundred_samples() {
        let mut latch = LateLatch::default();
        latch.committed(Duration::from_millis(8));
        for _ in 0..98 {
            latch.committed(Duration::from_millis(1));
        }
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(9500));
        latch.committed(Duration::from_millis(1));
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(2500));
    }

    #[test]
    fn miss_raises_budget_temporarily_and_dispatch_delay_cannot_cause_a_miss() {
        let at = Instant::now();
        let mut latch = LateLatch::default();
        latch.flipped(Some(at), PERIOD, true);
        latch.committed(Duration::from_millis(2));
        assert_eq!(latch.flipped(Some(at + PERIOD), PERIOD, true), Some(true));
        latch.committed(Duration::from_millis(2));
        assert_eq!(
            latch.flipped(Some(at + PERIOD * 3), PERIOD, true),
            Some(false)
        );
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(7500));
        for _ in 0..100 {
            latch.committed(Duration::from_millis(2));
        }
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(7500));
        for _ in 0..8 {
            latch.committed(Duration::from_millis(2));
        }
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(3500));
    }

    #[test]
    fn only_pending_work_after_a_flip_can_defer_and_idle_resets_it() {
        let at = Instant::now();
        let mut latch = LateLatch::default();
        assert!(latch.plan(at, MARGIN).deadline.is_none());
        latch.committed(Duration::from_millis(1));
        assert!(latch.plan(at, MARGIN).deadline.is_none()); // submit is not a flip
        latch.flipped(Some(at), PERIOD, false);
        assert!(latch.plan(at, MARGIN).deadline.is_none()); // first request after idle
        latch.flipped(Some(at), PERIOD, true);
        assert!(latch.plan(at, MARGIN).deadline.is_some());
        latch.park();
        assert!(latch.plan(at, MARGIN).deadline.is_none());
        latch.flipped(None, PERIOD, true);
        assert!(latch.plan(at, MARGIN).deadline.is_none());
    }

    #[test]
    fn repeated_misses_back_off_to_immediate_and_outputs_have_distinct_targets() {
        let at = Instant::now();
        let mut fast = LateLatch::default();
        let mut slow = LateLatch::default();
        fast.flipped(Some(at), PERIOD / 2, true);
        slow.flipped(Some(at), PERIOD, true);
        assert_ne!(fast.plan(at, MARGIN).deadline, slow.plan(at, MARGIN).deadline);
        let mut flip = at;
        for _ in 0..3 {
            slow.committed(Duration::from_millis(2));
            flip += PERIOD * 2;
            assert_eq!(slow.flipped(Some(flip), PERIOD, true), Some(false));
        }
        assert!(slow.plan(flip, MARGIN).deadline.is_none());
        assert!(fast.plan(at, MARGIN).deadline.is_some(), "no global back-off");
    }

    #[test]
    fn a_mode_change_or_missing_timestamp_does_not_claim_a_hit() {
        let at = Instant::now();
        let mut latch = LateLatch::default();
        latch.flipped(Some(at), PERIOD, true);
        latch.committed(Duration::from_millis(1));
        assert_eq!(latch.flipped(Some(at + PERIOD * 2), PERIOD * 2, true), None);
        latch.committed(Duration::from_millis(1));
        assert_eq!(latch.flipped(None, PERIOD * 2, true), None);
    }
}
