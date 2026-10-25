// SPDX-License-Identifier: MIT OR Apache-2.0
//! A demand-driven safety net for root logic when no redraw arrives.
use std::time::{Duration, Instant};

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
    fn stalled_redraw_runs_once_then_sleeps() {
        let now = Instant::now();
        let mut wake = LogicWake::default();
        assert_eq!(wake.deadline(), None);
        wake.request(now);
        let deadline = wake.deadline().unwrap();
        assert!(!wake.take_due(now));
        wake.request(now + Duration::from_millis(50));
        assert_eq!(wake.deadline(), Some(deadline));
        assert!(wake.take_due(deadline));
        assert!(!wake.take_due(deadline + Duration::from_secs(60)));
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
}
