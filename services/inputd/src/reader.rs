// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The evdev reader, on its own blocking thread. Two modes:
//!
//! - **observe** (safe): no grab, no re-emit, nothing fired; logs what each
//!   key would do. The compositor still sees every key.
//! - **grab**: the interception path through [`Grabber`]. This is the
//!   dead-keyboard risk, and it is recoverable: the kernel releases the
//!   grab and destroys the uinput device (releasing its keys) when this
//!   process's fds close, on any death, SIGKILL included. `--grab-timeout`
//!   exits the process after a fixed time for the first supervised run.
//!
//! The grab loop sleeps in `poll(2)` on the device and, when it follows the
//! console, on the active-VT attribute (`POLLPRI | POLLERR`, which sysfs
//! raises on every VT switch). The only timeout is a pending grab retry.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use inputd_core::{Fire, Interceptor};
use inputd_schema::SideModifiers;
use tokio::sync::mpsc::UnboundedSender;

use crate::devices::{CONSOLE_ACTIVE, EvdevDevice, SysfsConsole, UinputDevice};
use crate::grab::{Console, EV_KEY, Grabber};
use crate::service::Shared;

/// A verb the reader resolved, for the Bus side to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiredVerb {
    pub verb: String,
    pub args: Option<serde_json::Value>,
    pub service: Option<String>,
}

impl From<Fire> for FiredVerb {
    fn from(fire: Fire) -> Self {
        Self {
            verb: fire.verb.as_str().to_string(),
            args: fire.args,
            service: fire.service,
        }
    }
}

/// Observe only. Blocks until the device fails.
pub fn run_observe(device: &str, resolver: Shared) -> io::Result<()> {
    let mut dev = EvdevDevice::open(device)?;
    let mut keys = Interceptor::new();
    keys.begin(SideModifiers::NONE);
    tracing::info!(device, "observe only: no grab, no re-emit, nothing fired");
    loop {
        wait_readable(dev.fd())?;
        for (kind, code, value) in dev.read_events()? {
            if kind != EV_KEY {
                continue;
            }
            let step = {
                let resolver = resolver.lock().expect("resolver poisoned");
                keys.key(&resolver, code, value)
            };
            if let Some(fire) = &step.fire {
                tracing::info!(code, value, verb = fire.verb.as_str(), "would fire");
            } else if !step.emit {
                tracing::info!(code, value, "would swallow");
            }
        }
    }
}

/// How the grab path runs.
pub struct GrabOptions {
    pub device: String,
    /// `None`: grab always.
    pub vts: Option<Vec<u16>>,
    pub timeout: Option<Duration>,
}

/// The interception path. Returns only on a device, uinput or console
/// error; the caller then exits, so systemd can start a fresh broker.
pub fn run_grab(
    options: GrabOptions,
    resolver: Shared,
    fire_tx: UnboundedSender<FiredVerb>,
) -> io::Result<()> {
    let dev = EvdevDevice::open(&options.device)?;
    // The re-emitter first, and once: if uinput fails nothing is grabbed,
    // and the device survives every later switch, since a compositor in a
    // container cannot see one created after it started.
    let out = UinputDevice::keyboard()?;
    let mut console = match options.vts {
        Some(_) => Some(SysfsConsole::open(Path::new(CONSOLE_ACTIVE))?),
        None => None,
    };
    // Let a compositor find the new device before the first grab, so the
    // first keys re-emitted are not lost. The keyboard works meanwhile.
    std::thread::sleep(Duration::from_millis(300));
    // Arm the watchdog before the first grab: process exit closes every fd
    // and so releases the grab, whatever the loop below is doing.
    if let Some(timeout) = options.timeout {
        std::thread::spawn(move || {
            std::thread::sleep(timeout);
            std::process::exit(0);
        });
    }
    tracing::info!(
        device = options.device,
        vts = ?options.vts,
        timeout_s = options.timeout.map(|t| t.as_secs()),
        "grab path: unbound keys re-emitted on inputd virtual keyboard; kill this process to release"
    );
    let mut grabber = Grabber::new(dev, out, options.vts);
    let now = Instant::now();
    match console.as_mut() {
        Some(console) => grabber.console(console.active()?, now)?,
        None => grabber.start(now)?,
    }
    loop {
        let dev_fd = grabber.device_mut().fd();
        let mut fds = vec![libc::pollfd {
            fd: dev_fd,
            events: libc::POLLIN,
            revents: 0,
        }];
        if let Some(console) = &console {
            fds.push(libc::pollfd {
                fd: console.fd(),
                events: libc::POLLPRI | libc::POLLERR,
                revents: 0,
            });
        }
        let timeout = grabber.deadline().map_or(-1, |at| {
            let wait = at.saturating_duration_since(Instant::now());
            // Round up, so a wake never comes before the deadline.
            i32::try_from(wait.as_millis() + 1).unwrap_or(i32::MAX)
        });
        // SAFETY: a valid array of pollfd of the given length.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        let now = Instant::now();
        if let (Some(console), Some(pfd)) = (console.as_mut(), fds.get(1))
            && pfd.revents & (libc::POLLPRI | libc::POLLERR) != 0
        {
            grabber.console(console.active()?, now)?;
        }
        let dev_events = fds[0].revents;
        if dev_events & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            let _ = grabber.shutdown();
            return Err(io::Error::other("input device went away"));
        }
        if dev_events & libc::POLLIN != 0 {
            let events = grabber.device_mut().read_events()?;
            let resolver = resolver.lock().expect("resolver poisoned");
            for (kind, code, value) in events {
                if let Some(fire) = grabber.event(&resolver, kind, code, value)? {
                    let _ = fire_tx.send(fire.into());
                }
            }
        }
        grabber.tick(now)?;
    }
}

/// Block until `fd` is readable.
fn wait_readable(fd: libc::c_int) -> io::Result<()> {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut pfd, 1, -1) };
        if ready >= 0 {
            if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(io::Error::other("input device went away"));
            }
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
