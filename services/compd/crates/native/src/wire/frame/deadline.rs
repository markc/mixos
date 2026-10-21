// SPDX-License-Identifier: MIT OR Apache-2.0
//! One reusable, normally disabled timer per native output. No periodic idle wake.

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Dispatcher, LoopHandle, RegistrationToken};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

/// Readability on an exported sync_file means the GPU fence has signalled.
/// Record its observation without borrowing the renderer or waiting on the GPU.
/// The observation includes event dispatch latency, a conservative upper bound.
pub struct GpuCompletion<D: 'static> {
    elapsed: Rc<Cell<Option<std::time::Duration>>>,
    token: RegistrationToken,
    handle: LoopHandle<'static, D>,
}

impl<D: 'static> GpuCompletion<D> {
    pub fn new(
        handle: &LoopHandle<'static, D>,
        fd: std::os::fd::OwnedFd,
        start: Instant,
    ) -> Option<Self> {
        use smithay::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};
        let elapsed = Rc::new(Cell::new(None));
        let completed = elapsed.clone();
        let token = handle
            .insert_source(
                Generic::new(fd, Interest::READ, Mode::Level),
                move |_, _, _| {
                    completed.set(Some(start.elapsed()));
                    Ok(PostAction::Disable)
                },
            )
            .ok()?;
        Some(Self {
            elapsed,
            token,
            handle: handle.clone(),
        })
    }

    pub fn elapsed(&self) -> Option<std::time::Duration> {
        self.elapsed.get()
    }
}

impl<D: 'static> Drop for GpuCompletion<D> {
    fn drop(&mut self) {
        self.handle.remove(self.token);
    }
}

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
        // calloop 0.14's update invokes Timer::reregister, which registers the
        // new deadline even when this source was disabled after its last fire.
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
        assert!(
            !data.schedule.pending(),
            "b still waits for its own deadline"
        );
        assert!(b.arm(Instant::now()));
        event_loop
            .dispatch(Some(Duration::ZERO), &mut data)
            .unwrap();
        assert_eq!(data.fires, 2);
        assert!(data.schedule.pending());
    }

    #[test]
    fn update_rearms_a_timer_disabled_by_its_callback() {
        let mut event_loop: EventLoop<'static, Data> = EventLoop::try_new().unwrap();
        let mut data = Data::default();
        let timer = DeadlineTimer::new(&event_loop.handle(), "a".into(), fired).unwrap();
        for expected in 1..=2 {
            assert!(timer.arm(Instant::now()));
            event_loop
                .dispatch(Some(Duration::ZERO), &mut data)
                .unwrap();
            assert_eq!(data.fires, expected);
            event_loop
                .dispatch(Some(Duration::ZERO), &mut data)
                .unwrap();
            assert_eq!(data.fires, expected, "disabled between arms");
        }
    }

    #[test]
    fn gpu_readiness_is_observed_once_without_waiting_and_drop_removes_source() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        let mut event_loop: EventLoop<'static, ()> = EventLoop::try_new().unwrap();
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let start = Instant::now() - Duration::from_millis(3);
        let sample = GpuCompletion::new(&event_loop.handle(), reader.into(), start).unwrap();
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
        assert_eq!(sample.elapsed(), None);
        writer.write_all(&[1]).unwrap();
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
        assert!(sample.elapsed().unwrap() >= Duration::from_millis(3));
        let elapsed = sample.elapsed();
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
        assert_eq!(sample.elapsed(), elapsed, "readiness disables the source");
        drop(sample);
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();

        let (reader, _writer) = UnixStream::pair().unwrap();
        let sample =
            GpuCompletion::new(&event_loop.handle(), reader.into(), Instant::now()).unwrap();
        drop(sample); // reset/prune before completion
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
    }
}
