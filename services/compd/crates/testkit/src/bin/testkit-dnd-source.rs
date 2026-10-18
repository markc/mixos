// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! testkit-dnd-source: a minimal xdg toplevel that starts a Wayland
//! drag-and-drop on a left press, for compd's real-client checklist.
//!
//!   testkit-dnd-source --text NEEDLE --uri URI [--size WxH]
//!
//! Connects to `$WAYLAND_DISPLAY` and maps one xdg toplevel (default
//! 400x300). A left press on its left half starts `wl_data_device.start_drag`
//! with a source offering the text (`text/plain;charset=utf-8`,
//! `text/plain`); on its right half, a file as file managers offer one
//! (`text/uri-list` with URI, and URI as `text/plain`). The source answers
//! `send` with those bytes. One line per step, on stdout:
//!
//! ```text
//! PROBE mapped                     PROBE drag text|file
//! PROBE target <mime or ->         PROBE send <mime>
//! PROBE dropped                    PROBE finished
//! PROBE cancelled
//! ```
//!
//! Runs until the compositor closes it. Exits 2 on a setup or protocol error.

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, FromRawFd};
use std::os::unix::fs::FileExt;

use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_compositor::WlCompositor,
    wl_data_device::{self, WlDataDevice},
    wl_data_device_manager::{self, WlDataDeviceManager},
    wl_data_offer::WlDataOffer,
    wl_data_source::{self, WlDataSource},
    wl_pointer::{self, WlPointer},
    wl_registry::{self, WlRegistry},
    wl_seat::{self, WlSeat},
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child,
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

/// `BTN_LEFT`.
const LEFT: u32 = 0x110;

#[derive(Default)]
struct State {
    globals: Vec<(u32, String, u32)>,
    pending_serial: Option<u32>,
    size: Option<(i32, i32)>,
    drawn: (i32, i32),
    closed: bool,
    seat: Option<WlSeat>,
    pointer: Option<WlPointer>,
    pointer_x: f64,
    /// A left press not yet turned into a drag: its serial and half.
    press: Option<(u32, bool)>,
    text: String,
    uri: String,
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "PROBE {line}");
    let _ = out.flush();
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals.push((name, interface, version));
        }
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for State {
    fn event(
        state: &mut Self,
        _: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            state.pending_serial = Some(serial);
        }
    }
}

impl Dispatch<XdgToplevel, ()> for State {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                state.size = Some((width, height))
            }
            xdg_toplevel::Event::Close => state.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && caps.contains(wl_seat::Capability::Pointer)
            && state.pointer.is_none()
        {
            state.pointer = Some(seat.get_pointer(qh, ()));
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface_x, .. } => state.pointer_x = surface_x,
            wl_pointer::Event::Motion { surface_x, .. } => state.pointer_x = surface_x,
            wl_pointer::Event::Button {
                serial,
                button: LEFT,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => {
                let file = state.pointer_x >= f64::from(state.drawn.0) / 2.0;
                state.press = Some((serial, file));
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, bool> for State {
    fn event(
        state: &mut Self,
        _: &WlDataSource,
        event: wl_data_source::Event,
        file: &bool,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Target { mime_type } => {
                say(&format!("target {}", mime_type.as_deref().unwrap_or("-")));
            }
            wl_data_source::Event::Send { mime_type, fd } => {
                say(&format!("send {mime_type}"));
                let body = if *file {
                    if mime_type == "text/uri-list" {
                        format!("{}\r\n", state.uri)
                    } else {
                        state.uri.clone()
                    }
                } else {
                    state.text.clone()
                };
                let _ = File::from(fd).write_all(body.as_bytes());
            }
            wl_data_source::Event::DndDropPerformed => say("dropped"),
            wl_data_source::Event::DndFinished => say("finished"),
            wl_data_source::Event::Cancelled => say("cancelled"),
            _ => {}
        }
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataDevice,
        _: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlDataDeviceManager);
delegate_noop!(State: ignore WlDataOffer);

fn die(msg: &str) -> ! {
    eprintln!("testkit-dnd-source: {msg}");
    std::process::exit(2);
}

/// One XRGB8888 buffer, `w`×`h`: blue left half (text), green right (file).
fn buffer(shm: &WlShm, qh: &QueueHandle<State>, w: i32, h: i32) -> WlBuffer {
    let stride = w * 4;
    let size = (stride * h) as usize;
    let fd = unsafe { libc::memfd_create(c"testkit-dnd-source".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        die(&format!(
            "memfd_create: {}",
            std::io::Error::last_os_error()
        ));
    }
    let file = unsafe { File::from_raw_fd(fd) };
    file.set_len(size as u64)
        .unwrap_or_else(|e| die(&format!("size the pool: {e}")));
    let row: Vec<u8> = (0..w)
        .flat_map(|x| {
            let colour: u32 = if x < w / 2 { 0x001d_4ed8 } else { 0x0004_7857 };
            colour.to_le_bytes()
        })
        .collect();
    for y in 0..h {
        file.write_all_at(&row, (y * stride) as u64)
            .unwrap_or_else(|e| die(&format!("fill the pool: {e}")));
    }
    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(0, w, h, stride, wl_shm::Format::Xrgb8888, qh, ());
    pool.destroy();
    buffer
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("--version" | "-V")) {
        println!("testkit-dnd-source {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let mut state = State::default();
    let mut size = (400, 300);
    let mut i = 0;
    while i < args.len() {
        let value = || {
            args.get(i + 1)
                .cloned()
                .unwrap_or_else(|| die("a flag needs a value"))
        };
        match args[i].as_str() {
            "--text" => state.text = value(),
            "--uri" => state.uri = value(),
            "--size" => {
                let v = value();
                let (w, h) = v.split_once('x').unwrap_or_else(|| die("--size needs WxH"));
                size = (
                    w.parse().unwrap_or_else(|_| die("--size needs WxH")),
                    h.parse().unwrap_or_else(|_| die("--size needs WxH")),
                );
            }
            other => die(&format!("unexpected argument {other}")),
        }
        i += 2;
    }

    let conn = Connection::connect_to_env().unwrap_or_else(|e| die(&format!("connect: {e}")));
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    let registry = conn.display().get_registry(&qh, ());
    queue
        .roundtrip(&mut state)
        .unwrap_or_else(|e| die(&format!("roundtrip: {e}")));
    let find = |state: &State, interface: &str| {
        state
            .globals
            .iter()
            .find(|(_, name, _)| name == interface)
            .map(|(name, _, version)| (*name, *version))
    };
    let need = |state: &State, interface: &str| {
        find(state, interface).unwrap_or_else(|| die(&format!("no {interface} global")))
    };
    let (name, version) = need(&state, "wl_compositor");
    let compositor: WlCompositor = registry.bind(name, version.min(4), &qh, ());
    let (name, _) = need(&state, "wl_shm");
    let shm: WlShm = registry.bind(name, 1, &qh, ());
    let (name, version) = need(&state, "xdg_wm_base");
    let base: XdgWmBase = registry.bind(name, version.min(2), &qh, ());
    let (name, version) = need(&state, "wl_seat");
    let seat: WlSeat = registry.bind(name, version.min(5), &qh, ());
    let (name, version) = need(&state, "wl_data_device_manager");
    let manager: WlDataDeviceManager = registry.bind(name, version.min(3), &qh, ());
    let device = manager.get_data_device(&seat, &qh, ());
    state.seat = Some(seat);

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("testkit-dnd-source".into());
    toplevel.set_title("testkit-dnd-source".into());
    surface.commit();

    let mut mapped = false;
    loop {
        queue
            .blocking_dispatch(&mut state)
            .unwrap_or_else(|e| die(&format!("dispatch: {e}")));
        if state.closed {
            return;
        }
        if let Some(serial) = state.pending_serial.take() {
            xdg_surface.ack_configure(serial);
            let (w, h) = match state.size {
                Some((w, h)) if w > 0 && h > 0 => (w, h),
                _ => size,
            };
            let buffer = buffer(&shm, &qh, w, h);
            surface.attach(Some(&buffer), 0, 0);
            surface.damage_buffer(0, 0, w, h);
            surface.commit();
            state.drawn = (w, h);
            if !mapped {
                mapped = true;
                say("mapped");
            }
        }
        if let Some((serial, file)) = state.press.take() {
            let source = manager.create_data_source(&qh, file);
            if file {
                source.offer("text/uri-list".into());
            } else {
                source.offer("text/plain;charset=utf-8".into());
            }
            source.offer("text/plain".into());
            if source.version() >= 3 {
                source.set_actions(wl_data_device_manager::DndAction::Copy);
            }
            device.start_drag(Some(&source), &surface, None, serial);
            say(if file { "drag file" } else { "drag text" });
        }
    }
}
