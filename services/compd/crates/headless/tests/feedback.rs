// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Read default DMA-BUF feedback over the real headless Wayland socket.
#[path = "support/process.rs"]
mod process;

use std::{
    fs::{self, File},
    os::unix::{
        fs::{FileExt, MetadataExt},
        net::UnixStream,
    },
    thread,
    time::{Duration, Instant},
};
use wayland_client::{Connection, Dispatch, QueueHandle, protocol::wl_registry};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_dmabuf_feedback_v1::{self, ZwpLinuxDmabufFeedbackV1},
    zwp_linux_dmabuf_v1::{self, ZwpLinuxDmabufV1},
};

#[derive(Default)]
struct Feedback {
    version: Option<u32>,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    feedback: Option<ZwpLinuxDmabufFeedbackV1>,
    main_device: Option<libc::dev_t>,
    table: Vec<u8>,
    tranche: Vec<u16>,
    tranches: Vec<Vec<u16>>,
    done: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Feedback {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "zwp_linux_dmabuf_v1"
        {
            state.version = Some(version);
            assert!(
                version >= 4,
                "headless needs device feedback, got v{version}"
            );
            let dmabuf: ZwpLinuxDmabufV1 = registry.bind(name, version.min(5), qh, ());
            state.feedback = Some(dmabuf.get_default_feedback(qh, ()));
            state.dmabuf = Some(dmabuf);
        }
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for Feedback {
    fn event(
        _: &mut Self,
        _: &ZwpLinuxDmabufV1,
        _: zwp_linux_dmabuf_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for Feedback {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_linux_dmabuf_feedback_v1::Event;
        match event {
            Event::MainDevice { device } => {
                state.main_device = Some(libc::dev_t::from_ne_bytes(device.try_into().unwrap()));
            }
            Event::FormatTable { fd, size } => {
                state.table.resize(size as usize, 0);
                File::from(fd).read_exact_at(&mut state.table, 0).unwrap();
            }
            Event::TrancheFormats { indices } => {
                assert_eq!(indices.len() % 2, 0);
                state.tranche.extend(
                    indices
                        .chunks_exact(2)
                        .map(|index| u16::from_ne_bytes([index[0], index[1]])),
                );
            }
            Event::TrancheDone => state.tranches.push(std::mem::take(&mut state.tranche)),
            Event::Done => state.done = true,
            _ => {}
        }
    }
}

#[test]
#[ignore = "needs a GPU render node and built compd with backend-headless"]
fn default_feedback_names_render_device_and_offers_formats() {
    let node =
        std::env::var("COMPD_TEST_RENDER_NODE").unwrap_or_else(|_| "/dev/dri/renderD128".into());
    // Exercise explicit selection too: the process helper deliberately writes
    // an invalid configured node when an explicit node is supplied.
    let mut process = process::spawn(1, Some(&node), "feedback", None);
    let deadline = Instant::now() + Duration::from_secs(25);
    let stream = loop {
        if let Ok(stream) = UnixStream::connect(process.root.join("wayland-headless-test")) {
            break stream;
        }
        assert!(process.child.try_wait().unwrap().is_none(), "compd exited");
        assert!(Instant::now() < deadline, "headless socket timed out");
        thread::sleep(Duration::from_millis(10));
    };
    // The expectation is the node compd SELECTED (its EGL device), read from
    // its log, not the one requested: on a multi-GPU host they may differ.
    // The socket can exist before the backend has logged; wait for the line.
    let selected = loop {
        let found = fs::read_to_string(process.root.join("compd.log"))
            .unwrap_or_default()
            .lines()
            .find_map(|line| line.split("headless: EGL device = ").nth(1).map(str::trim).map(String::from));
        if let Some(path) = found {
            break path;
        }
        assert!(Instant::now() < deadline, "compd never logged its EGL device");
        thread::sleep(Duration::from_millis(10));
    };
    let expected = fs::metadata(&selected).unwrap().rdev();
    let connection = Connection::from_socket(stream).unwrap();
    let mut queue = connection.new_event_queue::<Feedback>();
    let _registry = connection.display().get_registry(&queue.handle(), ());
    let mut state = Feedback::default();
    queue.roundtrip(&mut state).unwrap();
    queue.roundtrip(&mut state).unwrap();
    assert!(state.version.is_some(), "missing DMA-BUF global");
    assert!(state.done, "incomplete default feedback");
    assert_eq!(state.main_device, Some(expected));
    assert!(!state.table.is_empty());
    assert_eq!(state.table.len() % 16, 0);
    let indices: Vec<_> = state.tranches.iter().flatten().collect();
    assert!(!indices.is_empty(), "no tranche carrying formats");
    for index in &indices {
        assert!(
            usize::from(**index) < state.table.len() / 16,
            "invalid format index"
        );
    }
    println!(
        "DMABUF_FEEDBACK v{} main_device={:#x} expected={expected:#x} requested={node} selected={selected}; {} tranches, {} format indices, {} table entries",
        state.version.unwrap(),
        state.main_device.unwrap(),
        state.tranches.len(),
        indices.len(),
        state.table.len() / 16
    );
    let log = fs::read_to_string(process.root.join("compd.log")).unwrap();
    let selection = log
        .lines()
        .find(|line| line.contains("dmabuf global with default feedback"))
        .unwrap();
    assert!(
        selection.contains("from backend composite node"),
        "{selection}"
    );
    println!("{selection}");
}
