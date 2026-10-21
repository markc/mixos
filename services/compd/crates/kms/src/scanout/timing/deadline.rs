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
    floor: Duration,
    hits: u16,
    first_flip: bool,
    first_commit: bool,
    submitted_margin: Duration,
    sample_deferred: bool,
    submitted_deferred: bool,
    pub sample_start: Option<Instant>,
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
            floor: Duration::ZERO,
            hits: 0,
            first_flip: true,
            first_commit: true,
            submitted_margin: Duration::ZERO,
            sample_deferred: false,
            submitted_deferred: false,
            sample_start: None,
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
            .max(self.floor)
            .saturating_add(margin)
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
        // An uncommitted sample cannot carry over into the next flip cycle.
        self.abandon();
        let deferred = std::mem::take(&mut self.submitted_deferred);
        if std::mem::take(&mut self.first_flip) {
            // A reset can precede an old-mode flip. Learn no phase from it.
            self.submitted = None;
            self.park();
            return None;
        }
        let hit = self.submitted.take().and_then(|target| {
            at.filter(|_| target.refresh == refresh).map(|actual| {
                let hit = actual < target.vblank + refresh / 2;
                if hit {
                    self.hits += 1;
                    if self.hits == 100 {
                        self.floor = self.floor.saturating_sub(Duration::from_micros(50));
                        self.hits = 0;
                    }
                } else {
                    // Learn the hidden GPU tail as well as CPU/dispatch cost.
                    // Keep this floor when the windowed CPU maximum expires.
                    // Immediate frames would have missed without late latch too.
                    if deferred {
                        self.floor = self
                            .floor
                            .max(
                                self.submitted_budget
                                    .saturating_sub(self.submitted_margin)
                                    .saturating_add(refresh / 8),
                            )
                            .min(refresh);
                    }
                    self.hits = 0;
                }
                hit
            })
        });
        if hit.is_none() && deferred {
            self.hits = 0;
        }
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
        self.abandon();
    }

    /// A skipped render queues no flip to clear its sample. Keep the phase and
    /// learned history, but let the next render choose its own sample origin.
    pub fn abandon(&mut self) {
        self.sample_start = None;
        self.sample_deferred = false;
    }

    /// Preserve the armed deadline through expiry and executor dispatch. For a
    /// clamped plan, preserve the plan instant through any sibling renders.
    pub fn begin(&mut self, now: Instant, plan: Plan) {
        if self.sample_start.is_none() {
            self.sample_start = Some(plan.deadline.unwrap_or(now));
            self.sample_deferred = plan.deadline.is_some();
            self.budget_used = plan.budget;
        }
    }

    /// Modesets and the first commit after reset are not steady-state samples.
    /// Returns the sample origin for an optional asynchronous GPU observer.
    pub fn committed(&mut self, now: Instant, margin: Duration, modeset: bool) -> Option<Instant> {
        let first = std::mem::take(&mut self.first_commit);
        if first || modeset {
            self.submitted = None;
            self.submitted_deferred = false;
            self.park();
            return None;
        }
        let start = self.sample_start.take();
        self.submitted_deferred = std::mem::take(&mut self.sample_deferred);
        self.submitted = self.candidate.take();
        self.submitted_budget = self.budget_used;
        self.submitted_margin = margin;
        self.following_flip = false;
        self.armed = None;
        if let Some(start) = start {
            self.advance_sample();
            self.observe(now.saturating_duration_since(start));
        }
        start
    }

    /// Also accepts non-blocking GPU completion observations. Only commits
    /// advance the buckets; an additional GPU observation does not age history.
    pub fn observe(&mut self, elapsed: Duration) {
        self.current = self.current.max(elapsed);
    }

    fn advance_sample(&mut self) {
        self.samples += 1;
        if self.samples == 50 {
            self.previous = self.current;
            self.current = Duration::ZERO;
            self.samples = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PERIOD: Duration = Duration::from_millis(16);
    const MARGIN: Duration = Duration::from_micros(1500);

    fn ready(at: Instant) -> LateLatch {
        let mut latch = LateLatch::default();
        assert_eq!(latch.committed(at, MARGIN, false), None);
        assert_eq!(latch.flipped(Some(at), PERIOD, true), None);
        latch.flipped(Some(at), PERIOD, true);
        latch
    }

    fn submit(latch: &mut LateLatch, at: Instant, elapsed: Duration) {
        let plan = latch.plan(at, MARGIN);
        latch.begin(at, plan);
        let start = latch.sample_start.unwrap();
        assert_eq!(latch.committed(start + elapsed, MARGIN, false), Some(start));
    }

    #[test]
    fn prediction_uses_flip_not_dispatch_and_clamps_late_or_large_budgets() {
        let at = Instant::now();
        let latch = ready(at);
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
    fn slightly_future_flip_arms_the_next_refresh_with_the_seed_budget() {
        let observed = Instant::now();
        let now = Duration::from_secs(10);
        for offset in [Duration::from_micros(300), Duration::from_micros(1500)] {
            let at = super::super::vblank::vblank::trusted_anchor(
                observed, Some(now + offset), Some(now),
            );
            let mut latch = ready(observed);
            latch.flipped(at, PERIOD, true);
            let plan = latch.plan(observed, MARGIN);
            assert_eq!(plan.target.unwrap().vblank, observed + offset + PERIOD);
            assert_eq!(plan.budget, Duration::from_micros(3500));
            assert_eq!(plan.deadline, Some(observed + offset + PERIOD - plan.budget));
        }
    }

    #[test]
    fn deadline_sample_includes_dispatch_and_sibling_render_time() {
        let at = Instant::now();
        let mut a = ready(at);
        let mut b = ready(at + Duration::from_micros(200));
        let plan_a = a.plan(at, MARGIN);
        let plan_b = b.plan(at, MARGIN);
        a.begin(at, plan_a);
        b.begin(at, plan_b);
        // Both timers are delayed by 1 ms; A then takes 2 ms before B's 2 ms.
        let dispatch = plan_b.deadline.unwrap() + Duration::from_millis(1);
        a.begin(dispatch, a.plan(dispatch, MARGIN));
        b.begin(dispatch, b.plan(dispatch, MARGIN));
        a.committed(dispatch + Duration::from_millis(2), MARGIN, false);
        b.committed(dispatch + Duration::from_millis(4), MARGIN, false);
        assert_eq!(b.budget(MARGIN), Duration::from_micros(6500));
        assert_eq!(a.budget(MARGIN), Duration::from_micros(4700));
    }

    #[test]
    fn clamped_sample_keeps_the_plan_instant() {
        let at = Instant::now();
        let mut latch = ready(at);
        let now = at + PERIOD;
        let plan = latch.plan(now, MARGIN);
        assert_eq!(plan.deadline, None);
        latch.begin(now, plan);
        latch.begin(now + Duration::from_millis(3), latch.plan(now, MARGIN));
        latch.committed(now + Duration::from_millis(5), MARGIN, false);
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(6500));
    }

    #[test]
    fn maximum_expires_within_one_hundred_commits_and_gpu_does_not_age_it() {
        let at = Instant::now();
        let mut latch = ready(at);
        submit(&mut latch, at, Duration::from_millis(2));
        latch.observe(Duration::from_millis(8));
        for _ in 0..98 {
            submit(&mut latch, at, Duration::from_millis(1));
        }
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(9500));
        submit(&mut latch, at, Duration::from_millis(1));
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(2500));
    }

    #[test]
    fn misses_learn_a_floor_that_decays_only_after_one_hundred_consecutive_hits() {
        let mut at = Instant::now();
        let mut latch = ready(at);
        submit(&mut latch, at, Duration::from_millis(2));
        at += PERIOD * 2;
        assert_eq!(latch.flipped(Some(at), PERIOD, true), Some(false));
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(5500));
        // Commits without target feedback cannot unwind a learned floor.
        for _ in 0..200 {
            submit(&mut latch, at, Duration::from_millis(2));
        }
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(5500));
        latch.flipped(Some(at), PERIOD, true);
        for _ in 0..99 {
            submit(&mut latch, at, Duration::from_millis(2));
            at += PERIOD;
            assert_eq!(latch.flipped(Some(at), PERIOD, true), Some(true));
        }
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(5500));
        submit(&mut latch, at, Duration::from_millis(2));
        at += PERIOD;
        assert_eq!(latch.flipped(Some(at), PERIOD, true), Some(true));
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(5450));
        submit(&mut latch, at, Duration::from_millis(2));
        at += PERIOD * 2;
        assert_eq!(latch.flipped(Some(at), PERIOD, true), Some(false));
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(7450));
        for _ in 0..99 {
            submit(&mut latch, at, Duration::from_millis(2));
            at += PERIOD;
            latch.flipped(Some(at), PERIOD, true);
        }
        submit(&mut latch, at, Duration::from_millis(2));
        latch.flipped(None, PERIOD, true); // unknown deferred feedback breaks the hit run
        assert_eq!(latch.hits, 0);
        submit(&mut latch, at, Duration::from_millis(2));
        latch.flipped(Some(at + PERIOD), PERIOD, true);
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(7450));
    }

    #[test]
    fn first_flip_and_commit_after_reset_and_modesets_are_not_samples() {
        let at = Instant::now();
        let mut latch = LateLatch::default();
        // An old-mode completion may arrive before the first new commit.
        latch.flipped(Some(at), PERIOD, true);
        assert!(latch.plan(at, MARGIN).deadline.is_none());
        latch.begin(at, latch.plan(at, MARGIN));
        assert_eq!(latch.committed(at + PERIOD * 3, MARGIN, false), None);
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(3500));
        assert_eq!(latch.flipped(Some(at + PERIOD * 4), PERIOD, true), None);
        latch.begin(at, latch.plan(at, MARGIN));
        assert_eq!(latch.committed(at + PERIOD * 6, MARGIN, true), None);
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(3500));
        assert_eq!(latch.flipped(Some(at + PERIOD * 7), PERIOD, true), None);
    }

    #[test]
    fn a_hidden_gpu_tail_does_not_repeat_the_old_one_hundred_frame_miss_cycle() {
        let mut at = Instant::now();
        let mut latch = ready(at);
        submit(&mut latch, at, Duration::from_millis(2));
        at += PERIOD * 2;
        assert_eq!(latch.flipped(Some(at), PERIOD, true), Some(false));
        for _ in 0..1000 {
            // 2 ms CPU plus 3 ms hidden GPU/dispatch cost; no fence available.
            assert!(latch.budget(MARGIN) >= Duration::from_millis(5));
            submit(&mut latch, at, Duration::from_millis(2));
            at += PERIOD;
            assert_eq!(latch.flipped(Some(at), PERIOD, true), Some(true));
        }
        assert_eq!(latch.budget(MARGIN), Duration::from_millis(5));
    }

    #[test]
    fn only_pending_work_after_a_flip_can_defer_and_idle_resets_it() {
        let at = Instant::now();
        let mut latch = LateLatch::default();
        assert!(latch.plan(at, MARGIN).deadline.is_none());
        latch.committed(at, MARGIN, false);
        assert!(latch.plan(at, MARGIN).deadline.is_none());
        latch.flipped(Some(at), PERIOD, true);
        assert!(latch.plan(at, MARGIN).deadline.is_none());
        latch.flipped(Some(at), PERIOD, false);
        assert!(latch.plan(at, MARGIN).deadline.is_none());
        latch.flipped(Some(at), PERIOD, true);
        assert!(latch.plan(at, MARGIN).deadline.is_some());
        latch.park();
        assert!(latch.plan(at, MARGIN).deadline.is_none());
        latch.flipped(None, PERIOD, true);
        assert!(latch.plan(at, MARGIN).deadline.is_none());
    }

    #[test]
    fn repeated_misses_eventually_clamp_without_changing_a_sibling_budget() {
        let mut at = Instant::now();
        let fast = ready(at);
        let mut slow = ready(at);
        for _ in 0..100 {
            submit(&mut slow, at, Duration::from_millis(2));
            at += PERIOD * 2;
            assert_eq!(slow.flipped(Some(at), PERIOD, true), Some(false));
            assert!(slow.floor <= PERIOD);
        }
        assert_eq!(slow.floor, PERIOD);
        assert!(slow.plan(at, MARGIN).deadline.is_none());
        assert_eq!(fast.budget(MARGIN), Duration::from_micros(3500));
    }

    #[test]
    fn clamped_misses_do_not_raise_the_floor() {
        let mut at = Instant::now();
        let mut latch = ready(at);
        for _ in 0..100 {
            let now = at + PERIOD;
            assert!(latch.plan(now, MARGIN).deadline.is_none());
            submit(&mut latch, now, Duration::from_millis(2));
            at += PERIOD * 2;
            assert_eq!(latch.flipped(Some(at), PERIOD, true), Some(false));
            assert_eq!(latch.floor, Duration::ZERO);
        }
    }

    #[test]
    fn idle_gaps_and_unsampled_flips_allow_floor_recovery() {
        let mut at = Instant::now();
        let mut latch = ready(at);
        submit(&mut latch, at, Duration::from_millis(2));
        at += PERIOD * 2;
        assert_eq!(latch.flipped(Some(at), PERIOD, true), Some(false));
        let floor = latch.floor;
        for _ in 0..100 {
            latch.park();
            assert_eq!(latch.flipped(None, PERIOD, false), None);
            // First redraw after idle is immediate and has no submitted target.
            submit(&mut latch, at, Duration::from_millis(2));
            at += PERIOD;
            assert_eq!(latch.flipped(Some(at), PERIOD, true), None);
            submit(&mut latch, at, Duration::from_millis(2));
            at += PERIOD;
            assert_eq!(latch.flipped(Some(at), PERIOD, false), Some(true));
        }
        assert_eq!(latch.floor, floor - Duration::from_micros(50));
    }

    #[test]
    fn rate_cap_abandons_a_sample_without_a_flip_before_the_next_render() {
        let at = Instant::now();
        let mut latch = ready(at);
        latch.begin(at, latch.plan(at, MARGIN));
        assert!(latch.sample_deferred);
        latch.abandon();
        assert_eq!(latch.sample_start, None);
        assert!(!latch.sample_deferred);

        // The rate-cap wait queues nothing: no flip occurs before this retry.
        let retry = at + PERIOD * 2;
        let plan = latch.plan(retry, MARGIN);
        assert_eq!(plan.deadline, None);
        latch.begin(retry, plan);
        assert_eq!(latch.sample_start, Some(retry));
        assert_eq!(
            latch.committed(retry + Duration::from_millis(2), MARGIN, false),
            Some(retry)
        );
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(3500));
        // This immediate retry misses the old target, but must not learn a
        // deferred-frame floor from the abandoned sample's flag.
        assert_eq!(
            latch.flipped(Some(at + PERIOD * 3), PERIOD, true),
            Some(false)
        );
        assert_eq!(latch.floor, Duration::ZERO);
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(3500));
    }

    #[test]
    fn flip_discards_an_uncommitted_sample_before_the_next_render() {
        let at = Instant::now();
        let mut latch = ready(at);
        latch.begin(at, latch.plan(at, MARGIN));
        assert!(latch.sample_start.is_some());
        latch.flipped(Some(at + PERIOD), PERIOD, true);
        assert_eq!(latch.sample_start, None);
        let plan = latch.plan(at + PERIOD, MARGIN);
        latch.begin(at + PERIOD, plan);
        assert_eq!(latch.sample_start, plan.deadline);
        latch.committed(
            plan.deadline.unwrap() + Duration::from_millis(2),
            MARGIN,
            false,
        );
        assert_eq!(latch.budget(MARGIN), Duration::from_micros(3500));
    }

    #[test]
    fn a_mode_change_or_missing_timestamp_does_not_claim_a_hit() {
        let at = Instant::now();
        let mut latch = ready(at);
        submit(&mut latch, at, Duration::from_millis(1));
        assert_eq!(latch.flipped(Some(at + PERIOD * 2), PERIOD * 2, true), None);
        submit(&mut latch, at, Duration::from_millis(1));
        assert_eq!(latch.flipped(None, PERIOD * 2, true), None);
    }
}
