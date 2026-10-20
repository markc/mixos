// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Shm test client: four opaque quadrants and a controlled 16x16 cyan cursor.
use std::{fs::File, os::{fd::{AsFd, FromRawFd}, unix::{fs::FileExt, net::UnixStream}}, thread, time::{Duration, Instant}};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop, protocol::{wl_registry::{self, WlRegistry}, wl_compositor::WlCompositor, wl_surface::WlSurface, wl_shm::{self, WlShm}, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer, wl_seat::WlSeat, wl_pointer::{self, WlPointer}, wl_callback::{self, WlCallback}}};
use wayland_protocols::{xdg::shell::client::{xdg_wm_base::{self, XdgWmBase}, xdg_surface::{self, XdgSurface}, xdg_toplevel::{self, XdgToplevel}}, wp::presentation_time::client::{wp_presentation::{self, WpPresentation}, wp_presentation_feedback::{self, WpPresentationFeedback}}};

pub const COLOURS: [u32; 4] = [0xffd02030, 0xff20d040, 0xff3040d0, 0xffe0d020];
pub const CURSOR: [u8; 3] = [0, 255, 255];

#[derive(Default)]
pub struct State {
    globals: Vec<(u32, String, u32)>,
    configured: bool,
    pub size: (i32, i32),
    cursor: Option<WlSurface>,
    pub callbacks: u64,
    pub presented: u64,
    pub discarded: u64,
    pub clock: Option<u32>,
    pub times: Vec<u64>,
}

pub struct Pattern {
    conn: Connection,
    queue: EventQueue<State>,
    pub state: State,
    surface: WlSurface,
    shm: WlShm,
    presentation: WpPresentation,
    buffer: WlBuffer,
}

impl Pattern {
    pub fn new(stream: UnixStream) -> Self {
        stream.set_nonblocking(true).unwrap();
        let conn = Connection::from_socket(stream).unwrap();
        let mut queue = conn.new_event_queue::<State>();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state).unwrap();
        let find = |name: &str| state.globals.iter().find(|(_, interface, _)| interface == name).map(|(n,v,version)| (*n, v.clone(), *version)).unwrap();
        let (n, _, v) = find("wl_compositor");
        let compositor: WlCompositor = registry.bind(n, v.min(4), &qh, ());
        let (n, _, _) = find("wl_shm");
        let shm: WlShm = registry.bind(n, 1, &qh, ());
        let (n, _, _) = find("xdg_wm_base");
        let shell: XdgWmBase = registry.bind(n, 1, &qh, ());
        let (n, _, _) = find("wp_presentation");
        let presentation: WpPresentation = registry.bind(n, 1, &qh, ());
        let surface = compositor.create_surface(&qh, ());
        let xdg = shell.get_xdg_surface(&surface, &qh, ());
        let top = xdg.get_toplevel(&qh, ());
        top.set_app_id("headless-pattern".into());
        top.set_fullscreen(None);
        let cursor = compositor.create_surface(&qh, ());
        let cursor_buffer = buffer(&shm, &qh, 16, 16, |_, _| 0xff00ffff);
        cursor.attach(Some(&cursor_buffer), 0, 0);
        cursor.damage_buffer(0, 0, 16, 16);
        cursor.commit();
        state.cursor = Some(cursor);
        for (n, _, v) in state.globals.iter().filter(|(_, i, _)| i == "wl_seat") {
            let seat: WlSeat = registry.bind(*n, (*v).min(7), &qh, ());
            let _ = seat.get_pointer(&qh, ());
        }
        surface.commit();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !state.configured {
            queue.roundtrip(&mut state).unwrap();
            assert!(Instant::now() < deadline, "initial configure");
            thread::sleep(Duration::from_millis(5));
        }
        // xdg-shell may initially offer 0x0 (the client's choice), before
        // compd places the first buffer and sends its decided size.
        if state.size == (0,0) {state.size=(1280,720);}
        assert_eq!(state.size, (1280, 720));
        let (w,h) = state.size;
        let buffer = buffer(&shm, &qh, w, h, |x,y| COLOURS[usize::from(x >= w/2) + 2*usize::from(y >= h/2)]);
        let mut client = Self {conn, queue, state, surface, shm, presentation, buffer};
        client.commit(true);
        client
    }

    pub fn pump(&mut self) {
        self.conn.flush().unwrap();
        self.queue.dispatch_pending(&mut self.state).unwrap();
        if let Some(guard) = self.queue.prepare_read() {
            match guard.read() {
                Ok(_) => {},
                Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {},
                Err(e) => panic!("pattern read: {e}"),
            }
        }
        self.queue.dispatch_pending(&mut self.state).unwrap();
    }

    pub fn commit(&mut self, damage: bool) {
        if damage {
            self.surface.attach(Some(&self.buffer), 0, 0);
            self.surface.damage_buffer(0, 0, self.state.size.0, self.state.size.1);
        }
        self.surface.frame(&self.queue.handle(), ());
        self.presentation.feedback(&self.surface, &self.queue.handle(), ());
        self.surface.commit();
        self.conn.flush().unwrap();
    }

    pub fn wait_callback(&mut self, previous: u64) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.state.callbacks <= previous {
            self.pump();
            assert!(Instant::now() < deadline, "frame callback timed out");
            thread::sleep(Duration::from_millis(2));
        }
    }

    pub fn change_buffer(&mut self) {
        let (w,h) = self.state.size;
        self.buffer = buffer(&self.shm, &self.queue.handle(), w, h, |_,_| 0xffaa3377);
    }
}

fn buffer(shm: &WlShm, qh: &QueueHandle<State>, w: i32, h: i32, colour: impl Fn(i32,i32)->u32) -> WlBuffer {
    let fd = unsafe {libc::memfd_create(c"headless-pattern".as_ptr(), libc::MFD_CLOEXEC)};
    assert!(fd >= 0);
    let file = unsafe {File::from_raw_fd(fd)};
    file.set_len((w*h*4) as u64).unwrap();
    for y in 0..h {
        let row: Vec<_> = (0..w).flat_map(|x| colour(x,y).to_le_bytes()).collect();
        file.write_all_at(&row, (y*w*4) as u64).unwrap();
    }
    let pool = shm.create_pool(file.as_fd(), w*h*4, qh, ());
    let buffer = pool.create_buffer(0,w,h,w*4,wl_shm::Format::Argb8888,qh,());
    pool.destroy();
    buffer
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(state: &mut Self, _: &WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_registry::Event::Global {name,interface,version} = event {state.globals.push((name,interface,version));}
    }
}
impl Dispatch<XdgWmBase, ()> for State {
    fn event(_: &mut Self, base: &XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping {serial} = event {base.pong(serial);}
    }
}
impl Dispatch<XdgSurface, ()> for State {
    fn event(state: &mut Self, surface: &XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure {serial} = event {surface.ack_configure(serial);state.configured = true;}
    }
}
impl Dispatch<XdgToplevel, ()> for State {
    fn event(state: &mut Self, _: &XdgToplevel, event: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_toplevel::Event::Configure {width,height,..} = event {state.size = (width,height);}
    }
}
impl Dispatch<WlPointer, ()> for State {
    fn event(state: &mut Self, pointer: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_pointer::Event::Enter {serial,..} = event {pointer.set_cursor(serial,state.cursor.as_ref(),0,0);}
    }
}
impl Dispatch<WlCallback, ()> for State {
    fn event(state: &mut Self, _: &WlCallback, event: wl_callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_callback::Event::Done {..} = event {state.callbacks+=1;}
    }
}
impl Dispatch<WpPresentation, ()> for State {
    fn event(state: &mut Self, _: &WpPresentation, event: wp_presentation::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_presentation::Event::ClockId {clk_id} = event {state.clock=Some(clk_id);}
    }
}
impl Dispatch<WpPresentationFeedback, ()> for State {
    fn event(state: &mut Self, _: &WpPresentationFeedback, event: wp_presentation_feedback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wp_presentation_feedback::Event::Presented {tv_sec_hi,tv_sec_lo,tv_nsec, flags,..} => {
                assert_eq!(flags, wayland_client::WEnum::Value(wp_presentation_feedback::Kind::empty()));
                state.presented+=1;
                state.times.push((((tv_sec_hi as u64)<<32) | tv_sec_lo as u64)*1_000_000_000 + tv_nsec as u64);
            },
            wp_presentation_feedback::Event::Discarded => state.discarded+=1,
            _=>{},
        }
    }
}
delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlSeat);
