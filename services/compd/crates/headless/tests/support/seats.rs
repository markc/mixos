// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use std::{collections::BTreeMap, os::unix::net::UnixStream, path::Path};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{wl_registry, wl_seat},
};

#[derive(Default)]
struct Seats(Vec<(wl_seat::WlSeat, Option<String>, Option<wl_seat::Capability>)>);

impl Dispatch<wl_registry::WlRegistry, ()> for Seats {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event
            && interface == "wl_seat"
        {
            state.0.push((registry.bind(name, version.min(7), qh, ()), None, None));
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Seats {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let row = state.0.iter_mut().find(|row| row.0 == *seat).unwrap();
        match event {
            wl_seat::Event::Name { name } => row.1 = Some(name),
            wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } => row.2 = Some(caps),
            _ => {}
        }
    }
}

pub fn capabilities(root: &Path) -> BTreeMap<String, wl_seat::Capability> {
    let conn = Connection::from_socket(UnixStream::connect(root.join("wayland-headless-test")).unwrap()).unwrap();
    let mut queue = conn.new_event_queue::<Seats>();
    let _registry = conn.display().get_registry(&queue.handle(), ());
    let mut seats = Seats::default();
    queue.roundtrip(&mut seats).unwrap();
    queue.roundtrip(&mut seats).unwrap();
    seats.0.into_iter().map(|(_, name, caps)| (name.unwrap(), caps.unwrap())).collect()
}
