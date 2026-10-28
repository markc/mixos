// SPDX-License-Identifier: MIT OR Apache-2.0
//! A single event-driven Wayland connection. File descriptors cross only thread
//! channels; all inter-session traffic goes through the native Bus client.
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, mpsc};

use crate::model::Stamp;
use anyhow::{Result, anyhow, bail, ensure};
use tokio::sync::oneshot;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self as ext_device, ExtDataControlDeviceV1 as ExtDevice},
    ext_data_control_manager_v1::ExtDataControlManagerV1 as ExtManager,
    ext_data_control_offer_v1::{self as ext_offer, ExtDataControlOfferV1 as ExtOffer},
    ext_data_control_source_v1::{self as ext_source, ExtDataControlSourceV1 as ExtSource},
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self as wlr_device, ZwlrDataControlDeviceV1 as WlrDevice},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1 as WlrManager,
    zwlr_data_control_offer_v1::{self as wlr_offer, ZwlrDataControlOfferV1 as WlrOffer},
    zwlr_data_control_source_v1::{self as wlr_source, ZwlrDataControlSourceV1 as WlrSource},
};

#[derive(Clone)]
pub enum Offer {
    Ext(ExtOffer),
    Wlr(WlrOffer),
}
impl Offer {
    fn types(&self) -> Vec<String> {
        match self {
            Self::Ext(o) => o.data::<Mutex<Vec<String>>>(),
            Self::Wlr(o) => o.data::<Mutex<Vec<String>>>(),
        }
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default()
    }
    fn receive(&self, mime: String, fd: &OwnedFd) {
        match self {
            Self::Ext(o) => o.receive(mime, fd.as_fd()),
            Self::Wlr(o) => o.receive(mime, fd.as_fd()),
        }
    }
    fn destroy(&self) {
        match self {
            Self::Ext(o) => o.destroy(),
            Self::Wlr(o) => o.destroy(),
        }
    }
}

pub enum Event {
    Selection {
        primary: bool,
        offer: Option<Offer>,
        types: Vec<String>,
        sequence: u64,
    },
    Send {
        stamp: Stamp,
        primary: bool,
        mime: String,
        fd: OwnedFd,
    },
    Failed(String),
}

enum Command {
    Read {
        offer: Offer,
        mime: String,
        fd: OwnedFd,
    },
    Set {
        stamp: Stamp,
        types: Vec<String>,
        primary: bool,
        clear: bool,
        reply: oneshot::Sender<Result<u64>>,
    },
    Stop,
}

#[derive(Clone)]
pub struct Driver {
    commands: mpsc::SyncSender<Command>,
    wake: Arc<Mutex<UnixStream>>,
    pub protocol: &'static str,
}

impl Driver {
    pub fn start(seat_name: Option<String>) -> Result<(Self, tokio::sync::mpsc::Receiver<Event>)> {
        Self::start_connection(Connection::connect_to_env()?, seat_name)
    }

    /// The same production driver over an already connected Wayland socket.
    /// The compd testkit uses this without changing process-wide environment.
    pub fn start_connection(
        connection: Connection,
        seat_name: Option<String>,
    ) -> Result<(Self, tokio::sync::mpsc::Receiver<Event>)> {
        let (wake, mut wake_read) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        wake_read.set_nonblocking(true)?;
        let (commands, requests) = mpsc::sync_channel(64);
        let (events, received) = tokio::sync::mpsc::channel(128);
        let (ready, wait) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("clipd-wayland".into())
            .spawn(move || {
                let result = (|| -> Result<()> {
                    let (globals, mut queue) = registry_queue_init::<State>(&connection)?;
                    let qh = queue.handle();
                    let mut state = State {
                        events: events.clone(),
                        selected: [None, None],
                        seats: Vec::new(),
                        finished: false,
                        source: [None, None],
                        sequence: 0,
                    };
                    for global in globals
                        .contents()
                        .clone_list()
                        .iter()
                        .filter(|g| g.interface == "wl_seat")
                    {
                        let seat = globals.registry().bind::<WlSeat, _, _>(
                            global.name,
                            global.version.min(7),
                            &qh,
                            (),
                        );
                        state.seats.push((seat, String::new()));
                    }
                    queue.roundtrip(&mut state)?;
                    let seat = state
                        .seats
                        .iter()
                        .find(|(_, name)| {
                            seat_name
                                .as_ref()
                                .map_or_else(|| !name.contains("agent"), |wanted| wanted == name)
                        })
                        .map(|(s, _)| s.clone())
                        .ok_or_else(|| anyhow!("SEAT: no matching non-agent seat"))?;
                    // Never downgrade after an ext bind error when ext is advertised.
                    let (manager, device, protocol) = if globals
                        .contents()
                        .clone_list()
                        .iter()
                        .any(|g| g.interface == "ext_data_control_manager_v1")
                    {
                        let m: ExtManager = globals.bind(&qh, 1..=1, ())?;
                        let d = m.get_data_device(&seat, &qh, ());
                        (Manager::Ext(m), Device::Ext(d), "ext-data-control-v1")
                    } else {
                        let m: WlrManager = globals.bind(&qh, 2..=2, ())?;
                        let d = m.get_data_device(&seat, &qh, ());
                        (Manager::Wlr(m), Device::Wlr(d), "wlr-data-control-v1")
                    };
                    queue.roundtrip(&mut state)?;
                    let _ = ready.send(Ok(protocol));
                    loop {
                        queue.dispatch_pending(&mut state)?;
                        ensure!(!state.finished, "Wayland data-control device finished");
                        while let Ok(command) = requests.try_recv() {
                            match command {
                                Command::Stop => return Ok(()),
                                Command::Read { offer, mime, fd } => offer.receive(mime, &fd),
                                Command::Set {
                                    stamp,
                                    types,
                                    primary,
                                    clear,
                                    reply,
                                } => {
                                    let index = usize::from(primary);
                                    let data = SourceData { stamp, primary };
                                    let source = match &manager {
                                        Manager::Ext(m) => {
                                            let s = (!clear)
                                                .then(|| m.create_data_source(&qh, data.clone()));
                                            if let Some(s) = &s {
                                                for mime in types {
                                                    s.offer(mime);
                                                }
                                                s.offer(data.stamp.marker());
                                            }
                                            if let Device::Ext(d) = &device {
                                                if primary {
                                                    d.set_primary_selection(s.as_ref());
                                                } else {
                                                    d.set_selection(s.as_ref());
                                                }
                                            }
                                            s.map(Source::Ext)
                                        }
                                        Manager::Wlr(m) => {
                                            let s = (!clear)
                                                .then(|| m.create_data_source(&qh, data.clone()));
                                            if let Some(s) = &s {
                                                for mime in types {
                                                    s.offer(mime);
                                                }
                                                s.offer(data.stamp.marker());
                                            }
                                            if let Device::Wlr(d) = &device {
                                                if primary {
                                                    d.set_primary_selection(s.as_ref());
                                                } else {
                                                    d.set_selection(s.as_ref());
                                                }
                                            }
                                            s.map(Source::Wlr)
                                        }
                                    };
                                    // A source can be set only once. Destroy the previous one
                                    // on replacement even if cancellation has not arrived yet.
                                    if let Some(old) = state.source[index].take() {
                                        old.destroy();
                                    }
                                    state.source[index] = source;
                                    // Fence all selection events through the compositor's
                                    // application of this set before acknowledging it.
                                    let applied = queue
                                        .roundtrip(&mut state)
                                        .map(|_| state.sequence)
                                        .map_err(Into::into);
                                    let _ = reply.send(applied);
                                }
                            }
                        }
                        connection.flush()?;
                        let Some(guard) = queue.prepare_read() else {
                            continue;
                        };
                        let mut fds = [
                            libc::pollfd {
                                fd: guard.connection_fd().as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0,
                            },
                            libc::pollfd {
                                fd: wake_read.as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0,
                            },
                        ];
                        // SAFETY: the two descriptors and array remain valid for poll.
                        let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
                        if rc < 0 {
                            let error = std::io::Error::last_os_error();
                            if error.kind() == std::io::ErrorKind::Interrupted {
                                continue;
                            }
                            return Err(error.into());
                        }
                        if fds[1].revents != 0 {
                            let mut bytes = [0u8; 256];
                            loop {
                                match wake_read.read(&mut bytes) {
                                    Ok(0) => return Ok(()),
                                    Ok(_) => {}
                                    Err(error)
                                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                                    {
                                        break;
                                    }
                                    Err(error) => return Err(error.into()),
                                }
                            }
                        }
                        if fds[0].revents != 0 {
                            guard.read()?;
                        } else {
                            drop(guard);
                        }
                    }
                })();
                if let Err(error) = result {
                    let message = error.to_string();
                    let _ = ready.send(Err(anyhow!(message.clone())));
                    let _ = events.blocking_send(Event::Failed(message));
                }
            })?;
        let protocol = wait.recv_timeout(std::time::Duration::from_secs(5))??;
        Ok((
            Self {
                commands,
                wake: Arc::new(Mutex::new(wake)),
                protocol,
            },
            received,
        ))
    }

    fn submit(&self, command: Command) -> Result<()> {
        self.commands
            .try_send(command)
            .map_err(|_| anyhow!("WAYLAND_BUSY: clipboard queue full or closed"))?;
        match self.wake.lock().unwrap().write(&[1]) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn set(
        &self,
        selection: &crate::model::Selection,
        primary: bool,
        clear: bool,
    ) -> Result<u64> {
        let (reply, wait) = oneshot::channel();
        self.submit(Command::Set {
            stamp: selection.stamp.clone(),
            types: selection.mime_types.clone(),
            primary,
            clear,
            reply,
        })?;
        tokio::time::timeout(std::time::Duration::from_secs(3), wait).await??
    }

    pub async fn read(&self, offer: Offer, mime: String, cap: usize) -> Result<Vec<u8>> {
        let (read, write) = pipe()?;
        self.submit(Command::Read {
            offer,
            mime,
            fd: write,
        })?;
        read_fd(read, cap).await
    }

    pub fn stop(&self) {
        let _ = self.submit(Command::Stop);
    }
}

fn pipe() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    // SAFETY: pipe2 initialises both entries on success; OwnedFd takes each once.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn nonblocking(fd: &OwnedFd) -> Result<()> {
    // SAFETY: fcntl only changes flags on this owned descriptor.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

pub async fn read_fd(fd: OwnedFd, cap: usize) -> Result<Vec<u8>> {
    nonblocking(&fd)?;
    let fd = tokio::io::unix::AsyncFd::new(fd)?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 65536];
        loop {
            let mut ready = fd.readable().await?;
            match ready.try_io(|fd| {
                // SAFETY: buffer is writable for its full length.
                let n = unsafe {
                    libc::read(
                        fd.get_ref().as_raw_fd(),
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                    )
                };
                if n < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            }) {
                Ok(Ok(0)) => return Ok(bytes),
                Ok(Ok(n)) => {
                    ensure!(
                        bytes.len() + n <= cap,
                        "SIZE_CAP: selection exceeds byte limit"
                    );
                    bytes.extend_from_slice(&buffer[..n]);
                }
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        }
    })
    .await
    .map_err(|_| anyhow!("SOURCE_TIMEOUT: selection owner did not finish"))?
}

pub async fn write_fd(fd: OwnedFd, bytes: &[u8]) -> Result<()> {
    nonblocking(&fd)?;
    let fd = tokio::io::unix::AsyncFd::new(fd)?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut offset = 0;
        while offset < bytes.len() {
            let mut ready = fd.writable().await?;
            match ready.try_io(|fd| {
                // SAFETY: the suffix is readable and the descriptor is owned.
                let n = unsafe {
                    libc::write(
                        fd.get_ref().as_raw_fd(),
                        bytes[offset..].as_ptr().cast(),
                        bytes.len() - offset,
                    )
                };
                if n < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            }) {
                Ok(Ok(0)) => bail!("PASTE_CLOSED: zero-byte write"),
                Ok(Ok(n)) => offset += n,
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => continue,
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| anyhow!("PASTE_TIMEOUT: receiver stopped reading"))?
}

enum Manager {
    Ext(ExtManager),
    Wlr(WlrManager),
}
enum Device {
    Ext(ExtDevice),
    Wlr(WlrDevice),
}
enum Source {
    Ext(ExtSource),
    Wlr(WlrSource),
}
impl Source {
    fn destroy(self) {
        match self {
            Self::Ext(s) if s.is_alive() => s.destroy(),
            Self::Wlr(s) if s.is_alive() => s.destroy(),
            _ => {}
        }
    }
}
#[derive(Clone)]
struct SourceData {
    stamp: Stamp,
    primary: bool,
}
struct State {
    events: tokio::sync::mpsc::Sender<Event>,
    selected: [Option<Offer>; 2],
    seats: Vec<(WlSeat, String)>,
    finished: bool,
    source: [Option<Source>; 2],
    sequence: u64,
}
impl State {
    fn selection(&mut self, primary: bool, offer: Option<Offer>) {
        let index = usize::from(primary);
        if let Some(old) = self.selected[index].take() {
            old.destroy();
        }
        let types = offer.as_ref().map(Offer::types).unwrap_or_default();
        self.selected[index] = offer.clone();
        self.sequence += 1;
        // A full event queue is fatal: silently dropping a selection would leave
        // stale content visible on the Bus. systemd restarts the connection.
        if self
            .events
            .try_send(Event::Selection {
                primary,
                offer,
                types,
                sequence: self.sequence,
            })
            .is_err()
        {
            self.finished = true;
        }
    }
    fn send(&mut self, data: &SourceData, mime: String, fd: OwnedFd) {
        if mime == data.stamp.marker() {
            return;
        }
        let _ = self.events.try_send(Event::Send {
            stamp: data.stamp.clone(),
            primary: data.primary,
            mime,
            fd,
        });
    }
}
impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wayland_client::protocol::wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_seat::Event::Name { name } = event
            && let Some((_, old)) = state.seats.iter_mut().find(|(s, _)| s == seat)
        {
            *old = name;
        }
    }
}
macro_rules! dispatch_control {
    ($device:ty, $offer:ty, $source:ty, $de:ident, $oe:ident, $se:ident, $variant:ident) => {
        impl Dispatch<$device, ()> for State {
            fn event(state: &mut Self, _: &$device, event: $de::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
                match event {
                    $de::Event::Selection { id } => state.selection(false, id.map(Offer::$variant)),
                    $de::Event::PrimarySelection { id } => state.selection(true, id.map(Offer::$variant)),
                    $de::Event::Finished => state.finished = true,
                    _ => {}
                }
            }
            wayland_client::event_created_child!(State, $device, [0 => ($offer, Mutex::new(Vec::<String>::new()))]);
        }
        impl Dispatch<$offer, Mutex<Vec<String>>> for State {
            fn event(_: &mut Self, _: &$offer, event: $oe::Event, data: &Mutex<Vec<String>>, _: &Connection, _: &QueueHandle<Self>) {
                if let $oe::Event::Offer { mime_type } = event { let mut types = data.lock().unwrap(); if types.len() < 129 { types.push(mime_type); } }
            }
        }
        impl Dispatch<$source, SourceData> for State {
            fn event(state: &mut Self, source: &$source, event: $se::Event, data: &SourceData, _: &Connection, _: &QueueHandle<Self>) {
                match event { $se::Event::Send { mime_type, fd } => state.send(data, mime_type, fd), $se::Event::Cancelled if source.is_alive() => source.destroy(), _ => {} }
            }
        }
    }
}
dispatch_control!(
    ExtDevice, ExtOffer, ExtSource, ext_device, ext_offer, ext_source, Ext
);
dispatch_control!(
    WlrDevice, WlrOffer, WlrSource, wlr_device, wlr_offer, wlr_source, Wlr
);
wayland_client::delegate_noop!(State: ignore ExtManager);
wayland_client::delegate_noop!(State: ignore WlrManager);
