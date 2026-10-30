// SPDX-License-Identifier: MIT OR Apache-2.0
//! One reusable, normally disabled timer per native output. No periodic idle wake.

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Dispatcher, LoopHandle, RegistrationToken};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

/// Readability on an exported sync_file means the GPU fence has signalled.
/// Read the kernel signal time without borrowing the renderer or waiting on the
/// GPU. Callback dispatch latency must not train the render budget.
pub struct GpuCompletion<D: 'static> {
    elapsed: Rc<Cell<Option<std::time::Duration>>>,
    fired: Rc<Cell<bool>>,
    token: RegistrationToken,
    handle: LoopHandle<'static, D>,
}

impl<D: 'static> GpuCompletion<D> {
    pub fn new(
        handle: &LoopHandle<'static, D>,
        fd: std::os::fd::OwnedFd,
        start: Instant,
    ) -> Option<Self> {
        Self::with_query(
            handle,
            fd,
            start,
            kms::scanout::fence::completion::signalled_at,
        )
    }

    fn with_query(
        handle: &LoopHandle<'static, D>,
        fd: std::os::fd::OwnedFd,
        start: Instant,
        query: impl Fn(std::os::fd::BorrowedFd<'_>) -> Option<std::time::Duration> + 'static,
    ) -> Option<Self> {
        use smithay::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};
        use std::os::fd::AsFd;
        let elapsed = Rc::new(Cell::new(None));
        let completed = elapsed.clone();
        let fired = Rc::new(Cell::new(false));
        let signalled = fired.clone();
        let token = handle
            .insert_source(
                Generic::new(fd, Interest::READ, Mode::Level),
                move |_, fd, _| {
                    let signal = query(fd.as_fd());
                    let observed = Instant::now();
                    let now = kms::scanout::timing::vblank::monotonic_now();
                    completed
                        .set(signal.and_then(|signal| elapsed_at(start, observed, now?, signal)));
                    // Remove unregisters the fd exactly once; Drop skips this token.
                    signalled.set(true);
                    Ok(PostAction::Remove)
                },
            )
            .ok()?;
        Some(Self {
            elapsed,
            fired,
            token,
            handle: handle.clone(),
        })
    }

    pub fn elapsed(&self) -> Option<std::time::Duration> {
        self.elapsed.get()
    }

    #[cfg(test)]
    fn fired(&self) -> bool {
        self.fired.get()
    }

    #[cfg(test)]
    fn token(&self) -> RegistrationToken {
        self.token
    }
}

fn elapsed_at(
    start: Instant,
    observed: Instant,
    now: std::time::Duration,
    signal: std::time::Duration,
) -> Option<std::time::Duration> {
    // A future signal or one before this frame's origin is not a cost sample.
    observed
        .checked_duration_since(start)?
        .checked_sub(now.checked_sub(signal)?)
}

impl<D: 'static> Drop for GpuCompletion<D> {
    fn drop(&mut self) {
        // A fired source returned PostAction::Remove: calloop unregistered it
        // once and dropped it, and its token is stale. Removing a stale token
        // is a no-op today (tokens are versioned); skipping it keeps Drop from
        // ever touching a slot the loop may have reused.
        if !self.fired.get() {
            self.handle.remove(self.token);
        }
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
        // A Timer cancels a timer-wheel entry, not an epoll fd; disable-then-remove is safe.
        self.handle.remove(self.token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocols::redraw::schedule::{RedrawReason, Schedule};
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
        let observed = Instant::now();
        let now = kms::scanout::timing::vblank::monotonic_now().unwrap();
        let start = observed - Duration::from_millis(20);
        let signal = now - Duration::from_millis(17);
        // Model a 3 ms GPU completion whose callback is 17 ms late. Use the
        // real calloop source with a controlled timestamp query, without a GPU.
        let queries = Rc::new(Cell::new(0));
        let calls = queries.clone();
        let sample =
            GpuCompletion::with_query(&event_loop.handle(), reader.into(), start, move |_| {
                calls.set(calls.get() + 1);
                Some(signal)
            })
            .unwrap();
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
        assert_eq!(sample.elapsed(), None);
        writer.write_all(&[1]).unwrap();
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
        assert!(
            sample.elapsed().unwrap().abs_diff(Duration::from_millis(3)) < Duration::from_millis(1)
        );
        let elapsed = sample.elapsed();
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
        assert_eq!(sample.elapsed(), elapsed, "readiness removes the source");
        assert_eq!(queries.get(), 1);
        drop(sample);
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();

        let (reader, _writer) = UnixStream::pair().unwrap();
        let sample =
            GpuCompletion::new(&event_loop.handle(), reader.into(), Instant::now()).unwrap();
        drop(sample); // reset/prune before completion
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
    }

    #[test]
    fn fired_source_is_removed_once() {
        use smithay::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        let mut event_loop: EventLoop<'static, ()> = EventLoop::try_new().unwrap();
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let start = Instant::now();
        let queries = Rc::new(Cell::new(0));
        let calls = queries.clone();
        let sample =
            GpuCompletion::with_query(&event_loop.handle(), reader.into(), start, move |_| {
                calls.set(calls.get() + 1);
                None
            })
            .unwrap();
        writer.write_all(&[1]).unwrap();
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
        assert_eq!(queries.get(), 1);
        assert!(sample.fired(), "readiness ran the callback");
        // Removed, not merely disabled: a disabled source can be re-enabled
        // (and would be unregistered a second time by a later remove).
        assert!(
            event_loop.handle().enable(&sample.token()).is_err(),
            "the source is gone from the loop"
        );
        // Drop must take the no-remove branch.
        drop(sample);
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
        assert_eq!(queries.get(), 1, "the query ran exactly once");

        // A fresh source still dispatches, so the loop survived the removal.
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let reader_fd: std::os::fd::OwnedFd = reader.into();
        let dispatched = Rc::new(Cell::new(0));
        let seen = dispatched.clone();
        event_loop
            .handle()
            .insert_source(
                Generic::new(reader_fd, Interest::READ, Mode::Level),
                move |_, _, _| {
                    seen.set(seen.get() + 1);
                    Ok(PostAction::Disable)
                },
            )
            .unwrap();
        writer.write_all(&[1]).unwrap();
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
        assert_eq!(dispatched.get(), 1, "the loop still dispatches new sources");
    }

    #[test]
    fn gpu_cost_excludes_late_dispatch_and_rejects_unrelated_timestamps() {
        let start = Instant::now();
        let clock_start = Duration::from_secs(10);
        let cost = Duration::from_millis(3);
        for dispatch in [Duration::from_millis(4), Duration::from_millis(50)] {
            assert_eq!(
                elapsed_at(
                    start,
                    start + dispatch,
                    clock_start + dispatch,
                    clock_start + cost
                ),
                Some(cost)
            );
        }
        assert_eq!(
            elapsed_at(start, start + cost, clock_start + cost, clock_start - cost),
            None
        );
        assert_eq!(
            elapsed_at(start, start + cost, clock_start, clock_start + cost),
            None
        );
    }

    #[test]
    fn readable_fd_without_a_kernel_signal_timestamp_does_not_train_gpu_cost() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        let mut event_loop: EventLoop<'static, ()> = EventLoop::try_new().unwrap();
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let sample = GpuCompletion::new(
            &event_loop.handle(),
            reader.into(),
            Instant::now() - Duration::from_millis(50),
        )
        .unwrap();
        writer.write_all(&[1]).unwrap();
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
        assert_eq!(sample.elapsed(), None);
        drop(sample);
        event_loop.dispatch(Some(Duration::ZERO), &mut ()).unwrap();
    }
}
