// SPDX-License-Identifier: MIT OR Apache-2.0
//! A demand-driven safety net for root logic when no redraw arrives.
use core::time::Duration;
use std::time::Instant;

const GRACE: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(crate) struct LogicWake {
    deadline: Option<Instant>,
}

impl LogicWake {
    pub(crate) fn request(&mut self, when: Instant) {
        if let Some(deadline) = when.checked_add(GRACE) {
            // Further requests must not postpone an already waiting command.
            self.deadline = Some(self.deadline.map_or(deadline, |old| old.min(deadline)));
        }
    }

    pub(crate) fn cancel(&mut self) {
        self.deadline = None;
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(crate) fn take_due(&mut self, now: Instant) -> bool {
        if self.deadline.is_some_and(|deadline| deadline <= now) {
            self.cancel();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_is_consumed_once_and_requests_coalesce() {
        let now = Instant::now();
        let mut wake = LogicWake::default();
        assert_eq!(wake.deadline(), None);
        wake.request(now);
        let deadline = wake.deadline().expect("a repaint arms a deadline");
        assert!(!wake.take_due(now));
        wake.request(now + Duration::from_millis(50));
        assert_eq!(wake.deadline(), Some(deadline));
        assert!(wake.take_due(deadline));
        assert!(!wake.take_due(deadline + Duration::from_secs(1)));
        assert_eq!(wake.deadline(), None);
    }

    #[test]
    fn normal_redraw_cancels_fallback() {
        let now = Instant::now();
        let mut wake = LogicWake::default();
        wake.request(now);
        wake.cancel();
        assert!(!wake.take_due(now + Duration::from_secs(1)));
        assert_eq!(wake.deadline(), None);
    }

    #[test]
    fn delayed_request_is_not_run_early() {
        let now = Instant::now();
        let mut wake = LogicWake::default();
        wake.request(now + Duration::from_secs(30));
        assert!(!wake.take_due(now + Duration::from_secs(30)));
        assert!(wake.take_due(now + Duration::from_secs(31)));
    }

    #[test]
    fn later_redraw_can_arm_after_an_earlier_fallback_was_consumed() {
        let now = Instant::now();
        let mut wake = LogicWake::default();
        wake.request(now);
        assert!(wake.take_due(now + GRACE));
        // The event loop issues a previously queued delayed redraw five
        // seconds later. Its fallback belongs to this actual redraw time.
        let redraw = now + Duration::from_secs(5);
        wake.request(redraw);
        assert_eq!(wake.deadline(), Some(redraw + GRACE));
        let deadline = redraw + GRACE;
        let before = deadline.checked_sub(Duration::from_millis(1)).unwrap();
        assert!(!wake.take_due(before));
        assert!(wake.take_due(redraw + GRACE));
    }
}
