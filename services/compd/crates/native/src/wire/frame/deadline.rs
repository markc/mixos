// SPDX-License-Identifier: MIT OR Apache-2.0
//! One reusable, normally disabled timer per native output. No periodic idle wake.

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Dispatcher, LoopHandle, RegistrationToken};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

pub struct DeadlineTimer<D: 'static> {
    source: Dispatcher<'static, Timer, D>,
    token: RegistrationToken,
    handle: LoopHandle<'static, D>,
}

impl<D: 'static> DeadlineTimer<D> {
    pub fn new(
        handle: &LoopHandle<'static, D>,
        key: String,
        fired: fn(&mut D, &str),
    ) -> Option<Self> {
        let token_slot = Rc::new(Cell::new(None));
        let callback_token = token_slot.clone();
        let callback_handle = handle.clone();
        let source = Dispatcher::new(Timer::immediate(), move |at, _, data| {
            // calloop defers Disable until this callback returns; the source
            // stays allocated and its next arm only updates its registration.
            if let Some(token) = callback_token.get() {
                let _ = callback_handle.disable(&token);
            }
            fired(data, &key);
            TimeoutAction::ToInstant(at)
        });
        let token = handle.register_dispatcher(source.clone()).ok()?;
        token_slot.set(Some(token));
        if handle.disable(&token).is_err() {
            handle.remove(token);
            return None;
        }
        Some(Self {
            source,
            token,
            handle: handle.clone(),
        })
    }

    pub fn arm(&self, at: Instant) -> bool {
        self.source.as_source_mut().set_deadline(at);
        self.handle.update(&self.token).is_ok()
    }

    pub fn cancel(&self) {
        let _ = self.handle.disable(&self.token);
    }
}

impl<D: 'static> Drop for DeadlineTimer<D> {
    fn drop(&mut self) {
        self.handle.remove(self.token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocols::redraw::schedule::schedule::{RedrawReason, Schedule};
    use smithay::reexports::calloop::EventLoop;
    use std::time::Duration;

    #[derive(Default)]
    struct Data {
        schedule: Schedule,
        fires: u32,
    }

    fn fired(data: &mut Data, key: &str) {
        data.fires += 1;
        data.schedule.release(key);
    }

    #[test]
    fn reusable_timer_coalesces_reasons_and_leaves_siblings_alone() {
        let mut event_loop: EventLoop<'static, Data> = EventLoop::try_new().unwrap();
        let mut data = Data::default();
        data.schedule.rendering("a");
        data.schedule.rendering("b");
        data.schedule.frame("a", false);
        data.schedule.frame("b", false);
        data.schedule.queued("b");
        let timer = DeadlineTimer::new(&event_loop.handle(), "a".into(), fired).unwrap();
        for _ in 0..2 {
            data.schedule.defer("a");
            assert!(timer.arm(Instant::now() + Duration::from_millis(20)));
            data.schedule.request_for(RedrawReason::Cursor);
            data.schedule.request_for(RedrawReason::Ui);
            assert!(!data.schedule.pending());
            event_loop
                .dispatch(Some(Duration::ZERO), &mut data)
                .unwrap();
            assert!(!data.schedule.pending());
            event_loop
                .dispatch(Some(Duration::from_millis(100)), &mut data)
                .unwrap();
            assert!(data.schedule.pending());
            assert!(data.schedule.in_flight("b"));
            data.schedule.rendering("a");
            let reasons = data.schedule.frame("a", false);
            assert!(reasons.contains(RedrawReason::Cursor));
            assert!(reasons.contains(RedrawReason::Ui));
        }
        assert_eq!(data.fires, 2);
        event_loop
            .dispatch(Some(Duration::from_millis(25)), &mut data)
            .unwrap();
        assert_eq!(data.fires, 2, "disabled after firing");
        assert!(timer.arm(Instant::now()));
        timer.cancel();
        event_loop
            .dispatch(Some(Duration::ZERO), &mut data)
            .unwrap();
        assert_eq!(data.fires, 2, "cancelled timer stays silent");
        assert!(timer.arm(Instant::now()));
        drop(timer);
        event_loop
            .dispatch(Some(Duration::ZERO), &mut data)
            .unwrap();
        assert_eq!(data.fires, 2, "pruning removes the source");
    }

    #[test]
    fn two_deadlines_release_only_their_own_pipe() {
        let mut event_loop: EventLoop<'static, Data> = EventLoop::try_new().unwrap();
        let mut data = Data::default();
        data.schedule.register("a");
        data.schedule.register("b");
        data.schedule.defer("a");
        data.schedule.defer("b");
        let a = DeadlineTimer::new(&event_loop.handle(), "a".into(), fired).unwrap();
        let b = DeadlineTimer::new(&event_loop.handle(), "b".into(), fired).unwrap();
        assert!(a.arm(Instant::now()));
        assert!(b.arm(Instant::now() + Duration::from_secs(60)));
        event_loop
            .dispatch(Some(Duration::ZERO), &mut data)
            .unwrap();
        assert_eq!(data.fires, 1);
        data.schedule.rendering("a");
        assert!(!data.schedule.pending(), "b still waits for its own deadline");
        assert!(b.arm(Instant::now()));
        event_loop
            .dispatch(Some(Duration::ZERO), &mut data)
            .unwrap();
        assert_eq!(data.fires, 2);
        assert!(data.schedule.pending());
    }
}
