// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The test client's clipboard: `wl_data_device` on every bound seat, sources
//! that serve a fixed payload, and offers a test can read back through a pipe.
//!
//! Opt-in ([`TestClient::bind_data_devices`]): most tests never touch the
//! clipboard. Nothing here blocks — a read is a non-blocking pipe the harness
//! pumps until EOF ([`ClipboardRead::finish`]).

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, OwnedFd};

use wayland_client::protocol::{
    wl_data_device::{self, WlDataDevice},
    wl_data_device_manager::WlDataDeviceManager,
    wl_data_offer::{self, WlDataOffer},
    wl_data_source::{self, WlDataSource},
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use crate::client::{ClientState, TestClient};

/// What the clipboard handlers record, kept on [`ClientState`].
#[derive(Default)]
pub struct ClipboardState {
    /// Flavors each offer advertised, by the offer's protocol id.
    pub offer_mimes: HashMap<u32, Vec<String>>,
    /// The current selection offer per data device (index = seat index).
    pub selections: HashMap<usize, Option<WlDataOffer>>,
    /// Sources the compositor cancelled (replaced or cleared), by protocol id.
    pub cancelled: Vec<u32>,
    /// `Send` requests a source answered: `(source id, mime)`.
    pub served: Vec<(u32, String)>,
}

/// A paste in progress: the read end of the pipe handed to `wl_data_offer.receive`.
pub struct ClipboardRead {
    read: std::io::PipeReader,
    bytes: Vec<u8>,
}

impl ClipboardRead {
    /// Drain what has arrived; `true` once the writer closed the pipe.
    pub fn poll(&mut self) -> bool {
        let mut buf = [0u8; 4096];
        loop {
            match self.read.read(&mut buf) {
                Ok(0) => return true,
                Ok(n) => self.bytes.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => return false,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return true,
            }
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl TestClient {
    /// Bind `wl_data_device_manager` and get a data device on every bound seat
    /// (index = seat index, as [`TestClient::seat`]).
    pub fn bind_data_devices(&mut self) {
        let (name, version) = crate::client::find_in(&self.state.globals, "wl_data_device_manager");
        let manager: WlDataDeviceManager =
            self.registry().bind(name, version.min(3), self.qh(), ());
        let devices: Vec<WlDataDevice> = (0..self.seat_count())
            .map(|i| manager.get_data_device(self.seat(i), self.qh(), i))
            .collect();
        self.data_manager = Some(manager);
        self.data_devices = devices;
    }

    /// Offer `payload` under each of `mimes` as the clipboard on seat `seat`.
    /// The compositor only accepts it while this client has that seat's keyboard.
    pub fn set_selection(&mut self, seat: usize, mimes: &[&str], payload: &[u8]) -> WlDataSource {
        let manager = self.data_manager.as_ref().expect("bind_data_devices first");
        let source = manager.create_data_source(self.qh(), payload.to_vec());
        for mime in mimes {
            source.offer(mime.to_string());
        }
        self.data_devices[seat].set_selection(Some(&source), 0);
        source
    }

    /// The flavors of the clipboard currently offered on seat `seat`, if any.
    pub fn selection_mimes(&self, seat: usize) -> Option<Vec<String>> {
        let offer = self.state.clipboard.selections.get(&seat)?.as_ref()?;
        self.state.clipboard.offer_mimes.get(&offer.id().protocol_id()).cloned()
    }

    /// Ask for the clipboard on seat `seat` as `mime`; pump, then poll the read.
    pub fn receive(&mut self, seat: usize, mime: &str) -> ClipboardRead {
        let offer = self
            .state
            .clipboard
            .selections
            .get(&seat)
            .and_then(|o| o.clone())
            .expect("no clipboard offered on this seat");
        let (read, write) = std::io::pipe().expect("pipe");
        set_nonblocking(&read);
        offer.receive(mime.to_string(), write.as_fd());
        drop(write);
        ClipboardRead {
            read,
            bytes: Vec::new(),
        }
    }
}

fn set_nonblocking(read: &std::io::PipeReader) {
    use std::os::fd::AsRawFd;
    // SAFETY: plain fcntl on a descriptor this process owns.
    unsafe {
        let fd = read.as_raw_fd();
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
}

impl Dispatch<WlDataDeviceManager, ()> for ClientState {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, usize> for ClientState {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        seat: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_device::Event::Selection { id } = event {
            state.clipboard.selections.insert(*seat, id);
        }
    }

    wayland_client::event_created_child!(ClientState, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

impl Dispatch<WlDataOffer, ()> for ClientState {
    fn event(
        state: &mut Self,
        offer: &WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state
                .clipboard
                .offer_mimes
                .entry(offer.id().protocol_id())
                .or_default()
                .push(mime_type);
        }
    }
}

impl Dispatch<WlDataSource, Vec<u8>> for ClientState {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        payload: &Vec<u8>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                write_all(fd, payload);
                state
                    .clipboard
                    .served
                    .push((source.id().protocol_id(), mime_type));
            }
            wl_data_source::Event::Cancelled => {
                state.clipboard.cancelled.push(source.id().protocol_id());
            }
            _ => {}
        }
    }
}

/// Write the whole payload to the paste's pipe and close it. Test payloads are
/// small (well under a pipe buffer), so a blocking write cannot stall.
fn write_all(fd: OwnedFd, payload: &[u8]) {
    let mut file = std::fs::File::from(fd);
    let _ = file.write_all(payload);
}
