// SPDX-License-Identifier: MIT OR Apache-2.0
//! Monotonic clocks owned by the native-source generation.
use crate::{error::MixResult, native_events::refusal};
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub(crate) fn now() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // CLOCK_MONOTONIC is also the clock used by monotonic().
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
}

pub(crate) fn period(seconds: f64, repeating: bool) -> MixResult<()> {
    if !seconds.is_finite() || seconds <= 0.0 || seconds > 1e9 || (repeating && seconds < 0.1) {
        return Err(refusal("TIMER_PERIOD", "timer seconds must be finite, positive and at most 1e9; interval has a 0.1 s floor: use the frame clock for animation"));
    }
    Ok(())
}

pub(crate) struct Pending {
    pub tag: String,
    pub next: f64,
    pub period: Option<f64>,
}

impl Pending {
    pub fn take(&mut self, id: &str, now: f64) -> serde_json::Value {
        let due = self.next;
        let missed = self.period.map_or(0, |p| ((now - due) / p).floor().max(0.0) as u64);
        if let Some(p) = self.period {
            self.next = due + (missed + 1) as f64 * p;
        }
        serde_json::json!({"id": id, "tag": self.tag, "missed": missed, "due_at": due, "now": now})
    }
}

/// Same cancel-FD and join discipline as the other native readers. Readers
/// only wake the shared pump; delivery calculates one coalesced grid tick.
#[cfg(target_os = "linux")]
pub(crate) struct Source {
    cancel: OwnedFd,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
fn owned(fd: i32) -> MixResult<OwnedFd> {
    if fd < 0 {
        Err(refusal("TIMER_IO", std::io::Error::last_os_error().to_string()))
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

#[cfg(target_os = "linux")]
fn timespec(seconds: f64) -> libc::timespec {
    libc::timespec { tv_sec: seconds.floor() as _, tv_nsec: ((seconds.fract() * 1e9) as i64).max(1) as _ }
}

#[cfg(target_os = "linux")]
impl Source {
    pub fn start(queue: Arc<crate::native_events::Queue>, due: f64, period: Option<f64>) -> MixResult<Self> {
        let timer = owned(unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) })?;
        let spec = libc::itimerspec {
            it_value: timespec(due),
            it_interval: period.map(timespec).unwrap_or(libc::timespec { tv_sec: 0, tv_nsec: 0 }),
        };
        if unsafe { libc::timerfd_settime(timer.as_raw_fd(), libc::TFD_TIMER_ABSTIME, &spec, std::ptr::null_mut()) } < 0 {
            return Err(refusal("TIMER_IO", std::io::Error::last_os_error().to_string()));
        }
        let cancel = owned(unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) })?;
        let cancel_read = owned(unsafe { libc::fcntl(cancel.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) })?;
        let worker = std::thread::Builder::new().name("mix-timer".into()).spawn(move || {
            let mut fds = [libc::pollfd { fd: timer.as_raw_fd(), events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: cancel_read.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
            loop {
                if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted { continue; }
                    break;
                }
                if fds[1].revents != 0 { break; }
                if fds[0].revents != 0 {
                    let mut n = 0u64;
                    unsafe { libc::read(timer.as_raw_fd(), (&mut n as *mut u64).cast(), 8) };
                    queue.wake_timer();
                    if period.is_none() { break; }
                }
            }
        }).map_err(|e| refusal("TIMER_IO", e.to_string()))?;
        Ok(Self { cancel, worker: Some(worker) })
    }
}

#[cfg(target_os = "linux")]
impl Drop for Source {
    fn drop(&mut self) {
        let n = 1u64;
        unsafe { libc::write(self.cancel.as_raw_fd(), (&n as *const u64).cast(), 8) };
        if let Some(w) = self.worker.take() { let _ = w.join(); }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) struct Source;
#[cfg(not(target_os = "linux"))]
impl Source {
    pub fn start(_: Arc<crate::native_events::Queue>, _: f64, _: Option<f64>) -> MixResult<Self> {
        Err(refusal("TIMER_UNSUPPORTED", "timers require Linux timerfd; no polling fallback"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coalescing_and_slow_handler_preserve_grid() {
        let mut t = Pending { tag: "sample".into(), next: 10.0, period: Some(0.5) };
        let tick = t.take("timer:1", 11.7);
        assert_eq!(tick["missed"], 3);
        assert_eq!(tick["due_at"], 10.0);
        assert_eq!(t.next, 12.0);
        t.take("timer:1", 13.2);
        assert_eq!(t.next, 13.5);
    }
    #[test]
    fn floor_and_invalid_periods() {
        for n in [0.0, -1.0, 0.099, f64::NAN, f64::INFINITY] { assert!(period(n, true).is_err()); }
        assert!(period(0.1, true).is_ok());
        assert!(period(0.001, false).is_ok());
    }
}
