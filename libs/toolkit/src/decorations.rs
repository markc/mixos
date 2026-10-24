// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The native framing detector. It never borrows winit's connection, and
//! never calls `connect_to_env` (which would consume `WAYLAND_SOCKET`).
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use design::decorations::{Resolver, SessionFacts};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use wayland_client::protocol::{wl_callback, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle};

// Leave scheduling and reply overhead inside AD-5's 250 ms ceiling.
const BUDGET: Duration = Duration::from_millis(200);

/// Install once after winit has chosen the backend. Merely having a
/// WAYLAND_DISPLAY in the environment is not evidence for that choice.
pub fn initialise(display: &impl HasDisplayHandle) {
    let wayland = display
        .display_handle()
        .is_ok_and(|handle| matches!(handle.as_raw(), RawDisplayHandle::Wayland(_)));
    design::decorations::install(Arc::new(Resolver::new(
        move || detect(wayland),
        || std::env::var_os("MIXOS_DECORATIONS").map(|value| value.to_string_lossy().into_owned()),
        |warning| eprintln!("toolkit: warning: {warning}"),
    )));
}

/// `wayland` comes from the app's actual window backend. No display access
/// occurs on rule 1, nor when a desktop already determines rule 2.
pub fn detect(wayland: bool) -> SessionFacts {
    if !wayland {
        return detect_with(false, String::new(), false, None, bounded_probe);
    }
    let desktops = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let handed_off = std::env::var_os("WAYLAND_SOCKET").is_some();
    let display =
        PathBuf::from(std::env::var_os("WAYLAND_DISPLAY").unwrap_or_else(|| "wayland-0".into()));
    let path = if display.is_absolute() {
        Some(display)
    } else {
        std::env::var_os("XDG_RUNTIME_DIR").map(|runtime| PathBuf::from(runtime).join(display))
    };
    detect_with(true, desktops, handed_off, path, bounded_probe)
}

fn detect_with(
    wayland: bool,
    desktops: String,
    handed_off: bool,
    path: Option<PathBuf>,
    check_global: impl FnOnce(PathBuf) -> Option<bool>,
) -> SessionFacts {
    let mut facts = SessionFacts {
        wayland,
        desktops,
        decoration_global: None,
    };
    if wayland && !handed_off && design::decorations::automatic(&facts).reason != "rule2" {
        facts.decoration_global = path.and_then(check_global);
    }
    facts
}

fn bounded_probe(path: PathBuf) -> Option<bool> {
    let deadline = Instant::now() + BUDGET;
    let (send, receive) = mpsc::sync_channel(1);
    // A stalled compositor or library initialisation cannot stall
    // the UI or a Bus reply. Late results are discarded, never cached later.
    let worker = std::thread::Builder::new()
        .name("decoration-registry".into())
        .spawn(move || {
            let _ = send.send(probe(path, deadline));
        });
    if worker.is_err() {
        return None;
    }
    receive
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()
        .flatten()
}

#[derive(Default)]
struct Registry {
    advertised: bool,
    done: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Registry {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { interface, .. } = event {
            state.advertised |= interface == "zxdg_decoration_manager_v1";
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for Registry {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.done = true;
    }
}

fn probe(path: PathBuf, deadline: Instant) -> Option<bool> {
    let stream = connect(&path, deadline)?;
    let connection = Connection::from_socket(stream).ok()?;
    let mut queue = connection.new_event_queue::<Registry>();
    let handle = queue.handle();
    let _registry = connection.display().get_registry(&handle, ());
    let _sync = connection.display().sync(&handle, ());
    connection.flush().ok()?;
    let mut state = Registry::default();
    while Instant::now() < deadline {
        queue.dispatch_pending(&mut state).ok()?;
        if state.done {
            return Some(state.advertised);
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut fd = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout = i32::try_from(remaining.as_millis()).ok()?;
        // SAFETY: fd is a live, exclusively owned detector connection. The
        // pointer addresses one pollfd, and the timeout is bounded above.
        let ready = unsafe { libc::poll(&mut fd, 1, timeout) };
        if ready <= 0 {
            return None;
        }
        guard.read().ok()?;
    }
    None
}

fn connect(path: &std::path::Path, deadline: Instant) -> Option<UnixStream> {
    // SAFETY: an all-zero sockaddr_un is valid storage for a pathname socket.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return None;
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (destination, source) in address.sun_path.iter_mut().zip(bytes) {
        *destination = *source as libc::c_char;
    }
    // SAFETY: socket has no pointer arguments; a successful fd is owned below.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return None;
    }
    // SAFETY: fd is a new, uniquely owned Unix stream socket from socket().
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    let length = libc::socklen_t::try_from(length).ok()?;
    // SAFETY: address holds a terminated pathname, and length covers just
    // its family and that pathname, within the sockaddr_un allocation.
    let result = unsafe {
        libc::connect(
            fd,
            std::ptr::from_ref(&address).cast::<libc::sockaddr>(),
            length,
        )
    };
    if result != 0 {
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            return None;
        }
        let timeout = i32::try_from(
            deadline
                .saturating_duration_since(Instant::now())
                .as_millis(),
        )
        .ok()?;
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: pollfd describes the live owned socket, and one element.
        if unsafe { libc::poll(&mut pollfd, 1, timeout) } <= 0 {
            return None;
        }
        if stream.take_error().ok()?.is_some() {
            return None;
        }
    }
    Some(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use design::{Decorations, decorations::automatic};

    #[test]
    fn real_detector_without_a_wayland_backend_is_bounded() {
        // Explicit backend injection makes this independent of the worker's
        // environment. This path opens no socket and launches no process.
        let start = Instant::now();
        let result = automatic(&detect(false));
        assert_eq!((result.mode, result.reason), (Decorations::Server, "rule1"));
        assert!(start.elapsed() < Duration::from_millis(250));
    }

    #[test]
    fn silent_registry_times_out_within_the_reply_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("display");
        let _server = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let start = Instant::now();
        assert_eq!(bounded_probe(path), None);
        assert!(start.elapsed() < Duration::from_millis(250));
    }

    #[test]
    fn descriptor_handoff_and_house_desktop_skip_the_registry() {
        for (desktop, handed_off, reason) in [("KDE", true, "rule4"), ("KDE:MixOS", false, "rule2")]
        {
            let facts = detect_with(
                true,
                desktop.into(),
                handed_off,
                Some(PathBuf::from("unused-display")),
                |_| {
                    panic!("the registry must not be checked");
                },
            );
            assert_eq!(automatic(&facts).reason, reason);
        }
    }

    #[test]
    fn failed_registry_connection_uses_rule4() {
        let dir = tempfile::tempdir().unwrap();
        let facts = detect_with(
            true,
            "KDE".into(),
            false,
            Some(dir.path().join("missing-display")),
            bounded_probe,
        );
        assert_eq!(automatic(&facts).reason, "rule4");
    }
}
