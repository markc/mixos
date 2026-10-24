// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The native framing detector. It never borrows winit's connection, and
//! never calls `connect_to_env` (which would consume `WAYLAND_SOCKET`).
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use design::decorations::{Resolver, SessionFacts, prefers_client_side};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use wayland_client::backend::WaylandError;
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
    let environment =
        std::env::var_os("MIXOS_DECORATIONS").map(|value| value.to_string_lossy().into_owned());
    design::decorations::install(Arc::new(Resolver::new(
        move || detect(wayland),
        move || environment.clone(),
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
    if wayland && !handed_off && !prefers_client_side(&facts.desktops) {
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
    let stream = UnixStream::connect(path).ok()?;
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
            queue.dispatch_pending(&mut state).ok()?;
            continue;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match guard.read() {
            Ok(_) => {}
            Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {
                // read consumes the guard; sleeping avoids spinning during this one-shot probe.
                let remaining = deadline.saturating_duration_since(Instant::now());
                std::thread::sleep(Duration::from_millis(2).min(remaining));
            }
            Err(_) => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use design::{Decorations, decorations::automatic};
    use std::io::{Read, Write};

    fn read_new_id(stream: &mut UnixStream, opcode: u32) -> u32 {
        let mut request = [0_u8; 12];
        stream.read_exact(&mut request).unwrap();
        assert_eq!(u32::from_ne_bytes(request[..4].try_into().unwrap()), 1);
        let header = u32::from_ne_bytes(request[4..8].try_into().unwrap());
        assert_eq!((header >> 16, header & 0xffff), (12, opcode));
        u32::from_ne_bytes(request[8..].try_into().unwrap())
    }

    fn event(output: &mut Vec<u8>, object: u32, opcode: u16, arguments: &[u8]) {
        let size = u32::try_from(arguments.len() + 8).unwrap();
        output.extend_from_slice(&object.to_ne_bytes());
        output.extend_from_slice(&((size << 16) | u32::from(opcode)).to_ne_bytes());
        output.extend_from_slice(arguments);
    }

    fn global(output: &mut Vec<u8>, registry: u32, name: u32, interface: &str) {
        let mut arguments = Vec::new();
        arguments.extend_from_slice(&name.to_ne_bytes());
        let length = u32::try_from(interface.len() + 1).unwrap();
        arguments.extend_from_slice(&length.to_ne_bytes());
        arguments.extend_from_slice(interface.as_bytes());
        arguments.push(0);
        while arguments.len() % 4 != 0 {
            arguments.push(0);
        }
        arguments.extend_from_slice(&1_u32.to_ne_bytes());
        event(output, registry, 0, &arguments);
    }

    fn check_registry_response(decoration: bool) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("display");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let registry = read_new_id(&mut stream, 1);
            let callback = read_new_id(&mut stream, 0);
            // Ensure the client must retry a read before the reply arrives.
            std::thread::sleep(Duration::from_millis(10));
            let mut reply = Vec::new();
            global(&mut reply, registry, 1, "wl_compositor");
            if decoration {
                global(&mut reply, registry, 2, "zxdg_decoration_manager_v1");
            }
            event(&mut reply, callback, 0, &1_u32.to_ne_bytes());
            stream.write_all(&reply).unwrap();
            // Keep the connection live until the client consumes sync and disconnects.
            let mut byte = [0_u8; 1];
            let _ = stream.read(&mut byte);
        });
        let start = Instant::now();
        assert_eq!(bounded_probe(path), Some(decoration));
        assert!(start.elapsed() < Duration::from_millis(150));
        server.join().unwrap();
    }

    #[test]
    fn registry_with_decoration_global_is_detected() {
        check_registry_response(true);
    }

    #[test]
    fn registry_without_decoration_global_is_detected() {
        check_registry_response(false);
    }

    #[test]
    fn real_detector_without_a_wayland_backend_is_bounded() {
        // Explicit backend injection makes this independent of the worker's
        // environment. This path opens no socket and launches no process.
        // This module never starts a process: no child processes by construction.
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
