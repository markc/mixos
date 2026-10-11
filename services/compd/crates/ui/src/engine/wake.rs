// SPDX-License-Identifier: MIT OR Apache-2.0
//! Event-driven wakeups for compositor-owned UIs.
//!
//! egui asks for a repaint through the context's repaint callback, from the
//! compositor thread during a pass or from any other thread. An immediate
//! request sets the runtime's pending bit and pings the compositor loop; a
//! delayed one records the earliest instant, which the host reads as the
//! runtime's deadline and sleeps until. Nothing polls.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::ping::make_ping;

type Wake = Arc<dyn Fn() + Send + Sync>;

fn loop_waker() -> &'static RwLock<Option<Wake>> {
    static WAKER: OnceLock<RwLock<Option<Wake>>> = OnceLock::new();
    WAKER.get_or_init(|| RwLock::new(None))
}

/// Register before entering the loop. `request_frame` must schedule a UI
/// frame; merely waking the poller would leave a clean output parked.
pub fn register<D: 'static>(
    handle: &LoopHandle<'static, D>,
    request_frame: fn(&mut D),
) -> Result<(), String> {
    let (ping, source) = make_ping().map_err(|error| format!("UI wake source: {error}"))?;
    handle
        .insert_source(source, move |_, _, data| request_frame(data))
        .map_err(|error| format!("UI wake source: {error}"))?;
    *loop_waker()
        .write()
        .unwrap_or_else(|error| error.into_inner()) = Some(Arc::new(move || ping.ping()));
    Ok(())
}

/// Wake the compositor loop, if a waker is registered.
pub(crate) fn notify() {
    let wake = loop_waker()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if let Some(wake) = wake {
        wake();
    }
}

/// One runtime's repaint requests.
///
/// A suspended runtime (its surface is off screen) still records requests,
/// but they neither wake the loop nor count as pending: an animation nobody
/// can see must not keep the compositor drawing. Resuming reports whether any
/// arrived, so the runtime catches up with one pass.
#[derive(Clone)]
pub(crate) struct RuntimeWake {
    pending: Arc<AtomicBool>,
    at: Arc<Mutex<Option<Instant>>>,
    suspended: Arc<AtomicBool>,
    deferred: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl RuntimeWake {
    pub fn new() -> Self {
        Self::with_waker(notify)
    }

    pub(crate) fn with_waker(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            pending: Arc::new(AtomicBool::new(false)),
            at: Arc::new(Mutex::new(None)),
            suspended: Arc::new(AtomicBool::new(false)),
            deferred: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(wake),
        }
    }

    /// Record a repaint request `delay` from now and wake the loop.
    pub fn request(&self, delay: Duration) {
        if delay == Duration::MAX {
            // egui's "no repaint wanted".
            return;
        }
        if self.suspended.load(Ordering::Acquire) {
            self.deferred.store(true, Ordering::Release);
            return;
        }
        if delay.is_zero() {
            self.pending.store(true, Ordering::Release);
        } else if let Some(at) = Instant::now().checked_add(delay) {
            let mut slot = self.at.lock().unwrap_or_else(|error| error.into_inner());
            *slot = Some(slot.map_or(at, |earlier| earlier.min(at)));
        } else {
            // Too far away to represent: as good as never.
            return;
        }
        (self.wake)();
    }

    /// A repaint is wanted now: requested immediately, or a delayed request
    /// whose instant has arrived.
    pub fn pending(&self) -> bool {
        !self.suspended.load(Ordering::Acquire)
            && (self.pending.load(Ordering::Acquire) || self.due(Instant::now()))
    }

    /// Suspend or resume. Resuming returns whether a repaint was asked for
    /// while suspended.
    pub fn set_suspended(&self, suspended: bool) -> bool {
        self.suspended.store(suspended, Ordering::Release);
        !suspended && self.deferred.swap(false, Ordering::AcqRel)
    }

    /// Consume the request this pass answers.
    pub fn take(&self) -> bool {
        let immediate = self.pending.swap(false, Ordering::AcqRel);
        let now = Instant::now();
        let mut slot = self.at.lock().unwrap_or_else(|error| error.into_inner());
        let due = slot.is_some_and(|at| at <= now);
        if due {
            *slot = None;
        }
        immediate || due
    }

    /// The future instant a delayed request is waiting for.
    pub fn deadline(&self) -> Option<Instant> {
        if self.suspended.load(Ordering::Acquire) {
            return None;
        }
        let slot = self.at.lock().unwrap_or_else(|error| error.into_inner());
        slot.filter(|at| *at > Instant::now())
    }

    fn due(&self, now: Instant) -> bool {
        let slot = self.at.lock().unwrap_or_else(|error| error.into_inner());
        slot.is_some_and(|at| at <= now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::reexports::calloop::EventLoop;

    #[test]
    fn a_repaint_from_another_thread_wakes_the_loop_and_stays_pending() {
        let mut event_loop: EventLoop<'static, u32> = EventLoop::try_new().unwrap();
        let (ping, source) = make_ping().unwrap();
        let wake = RuntimeWake::with_waker(move || ping.ping());
        event_loop
            .handle()
            .insert_source(source, |_, _, wakes: &mut u32| {
                *wakes += 1;
            })
            .unwrap();
        let mut wakes = 0;
        let remote = wake.clone();
        let worker = std::thread::spawn(move || remote.request(Duration::ZERO));
        // The timeout bounds failure; there are no timers or input sources.
        event_loop
            .dispatch(Duration::from_secs(1), &mut wakes)
            .unwrap();
        worker.join().unwrap();
        assert_eq!(wakes, 1);
        assert!(wake.pending());
        assert!(wake.take());
        assert!(!wake.take());
    }

    #[test]
    fn a_delayed_request_is_a_deadline_until_it_arrives() {
        let wake = RuntimeWake::with_waker(|| {});
        wake.request(Duration::from_millis(30));
        wake.request(Duration::from_millis(500));
        let deadline = wake.deadline().expect("a deadline");
        assert!(deadline <= Instant::now() + Duration::from_millis(30));
        assert!(!wake.pending(), "not due yet");
        std::thread::sleep(Duration::from_millis(40));
        assert!(wake.pending());
        assert!(wake.take());
        assert_eq!(wake.deadline(), None);
    }

    #[test]
    fn a_suspended_runtime_is_not_woken_and_catches_up_on_resume() {
        let woken = Arc::new(AtomicBool::new(false));
        let flag = woken.clone();
        let wake = RuntimeWake::with_waker(move || flag.store(true, Ordering::Release));
        assert!(!wake.set_suspended(true));
        wake.request(Duration::ZERO);
        wake.request(Duration::from_millis(5));
        assert!(
            !woken.load(Ordering::Acquire),
            "an off-screen animation does not wake the loop"
        );
        assert!(!wake.pending());
        assert_eq!(wake.deadline(), None);
        assert!(
            wake.set_suspended(false),
            "resuming reports the missed request"
        );
        assert!(!wake.set_suspended(false), "once");
    }

    #[test]
    fn no_repaint_wanted_is_ignored() {
        let wake = RuntimeWake::with_waker(|| panic!("must not wake"));
        wake.request(Duration::MAX);
        assert!(!wake.pending());
        assert_eq!(wake.deadline(), None);
    }
}
