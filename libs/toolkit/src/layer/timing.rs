// SPDX-License-Identifier: MIT OR Apache-2.0
//! The loop's timing rules, kept pure so they can be tested without a
//! compositor: how long a skipped frame waits before its retry, whether a
//! frame callback has waited too long, and how long the loop may block.

use std::time::{Duration, Instant};

/// How soon a surface whose frame was skipped is drawn again (the first
/// retry; it doubles with each further skip).
pub(crate) const RETRY: Duration = Duration::from_millis(50);

/// How long a frame callback is waited for before the surface may draw
/// again anyway, and the cap on the retry backoff.
pub(crate) const WATCHDOG: Duration = Duration::from_secs(1);

/// The wait before the next attempt after `skips` frames skipped in a row:
/// 50 ms doubling, capped at one second.
pub(crate) fn retry_after(skips: u32) -> Duration {
    (RETRY * 2u32.saturating_pow(skips.min(5))).min(WATCHDOG)
}

/// Whether a frame callback asked for at `since` has waited out the
/// watchdog by `now`.
pub(crate) fn expired(since: Instant, now: Instant) -> bool {
    now.duration_since(since) >= WATCHDOG
}

/// What one surface asks of the loop's next wait: until its watchdog runs
/// out (a frame callback pending since `pending_since`), until its retry
/// deadline (a skipped frame), or nothing.
pub(crate) fn surface_wait(
    pending_since: Option<Instant>,
    retry_at: Option<Instant>,
    now: Instant,
) -> Option<Duration> {
    match pending_since {
        Some(since) => Some(WATCHDOG.saturating_sub(now.duration_since(since))),
        None => retry_at.map(|t| t.saturating_duration_since(now)),
    }
}

/// The loop's wait: the shortest any surface asks for, else none (block
/// until an event).
pub(crate) fn shortest(waits: impl IntoIterator<Item = Option<Duration>>) -> Option<Duration> {
    waits.into_iter().flatten().min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_retry_backs_off_to_one_second() {
        assert_eq!(retry_after(0), Duration::from_millis(50));
        assert_eq!(retry_after(1), Duration::from_millis(100));
        assert_eq!(retry_after(3), Duration::from_millis(400));
        assert_eq!(retry_after(4), Duration::from_millis(800));
        assert_eq!(retry_after(5), WATCHDOG);
        assert_eq!(retry_after(u32::MAX), WATCHDOG);
    }

    #[test]
    fn the_watchdog_expires_after_a_second() {
        let t = Instant::now();
        assert!(!expired(t, t));
        assert!(!expired(t, t + Duration::from_millis(999)));
        assert!(expired(t, t + WATCHDOG));
    }

    #[test]
    fn the_loop_waits_for_the_soonest_deadline_or_blocks() {
        let now = Instant::now();
        // Pending for 300 ms: 700 ms left on the watchdog.
        let pending = surface_wait(Some(now - Duration::from_millis(300)), None, now);
        assert_eq!(pending, Some(Duration::from_millis(700)));
        // A pending frame waits on its watchdog, not a retry.
        let both = surface_wait(Some(now), Some(now + Duration::from_millis(50)), now);
        assert_eq!(both, Some(WATCHDOG));
        // A retry deadline already past: no wait.
        assert_eq!(
            surface_wait(None, Some(now - Duration::from_millis(5)), now),
            Some(Duration::ZERO)
        );
        // An idle surface asks for nothing; idle everywhere blocks.
        assert_eq!(surface_wait(None, None, now), None);
        assert_eq!(shortest([None, None]), None);
        assert_eq!(
            shortest([None, Some(Duration::from_millis(80)), Some(Duration::from_millis(30))]),
            Some(Duration::from_millis(30))
        );
    }
}
