// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The compositor side of a cast: a Wayland client of compd's
//! `zwlr_screencopy_manager_v1` (version 3, shm only). One capture thread
//! owns one connection, asks for a damage copy of its output, hands each
//! ready frame to its sink and waits for the frame interval before asking
//! again. A static screen produces no frames: `copy_with_damage` is not
//! answered until something on the output changes.
use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use wayland_client::backend::WaylandError;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_output::{self, WlOutput},
    wl_registry::WlRegistry,
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

/// Consecutive failed copies tolerated before the capture gives up.
const MAX_FAILURES: u32 = 10;
/// How often a blocked wait looks at the stop flag.
const POLL_MS: i32 = 200;

/// One output as a cast source.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputInfo {
    pub name: String,
    pub description: String,
    /// Compositor (logical) position and size.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// The mode in physical pixels: the size of every captured frame.
    pub mode_width: i32,
    pub mode_height: i32,
}

/// One captured picture, BGRx, tightly packed rows of `stride` bytes.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub data: Vec<u8>,
}

struct Output {
    proxy: WlOutput,
    info: OutputInfo,
    logical: bool,
}

#[derive(Default)]
struct Copy {
    /// (wl_shm format, width, height, stride) of the shm layout offered.
    shm: Option<(u32, u32, u32, u32)>,
    buffer_done: bool,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct State {
    outputs: Vec<Output>,
    copy: Copy,
}

fn connect() -> anyhow::Result<(EventQueue<State>, State, wayland_client::globals::GlobalList)> {
    let connection = Connection::connect_to_env()
        .map_err(|error| anyhow::anyhow!("no compositor connection: {error}"))?;
    let (globals, mut queue) = registry_queue_init::<State>(&connection)?;
    let qh = queue.handle();
    let mut state = State::default();
    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" {
            let index = state.outputs.len();
            let proxy: WlOutput = globals
                .registry()
                .bind(global.name, global.version.min(4), &qh, index);
            state.outputs.push(Output {
                proxy,
                info: OutputInfo {
                    name: format!("output-{index}"),
                    ..OutputInfo::default()
                },
                logical: false,
            });
        }
    }
    if let Ok(manager) = globals.bind::<ZxdgOutputManagerV1, _, _>(&qh, 2..=3, ()) {
        for (index, output) in state.outputs.iter().enumerate() {
            manager.get_xdg_output(&output.proxy, &qh, index);
        }
    }
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    for output in &mut state.outputs {
        if !output.logical {
            output.info.width = output.info.mode_width;
            output.info.height = output.info.mode_height;
        }
    }
    Ok((queue, state, globals))
}

/// The outputs compd advertises now, in advertisement order.
pub fn outputs() -> anyhow::Result<Vec<OutputInfo>> {
    let (_queue, state, _globals) = connect()?;
    Ok(state.outputs.into_iter().map(|output| output.info).collect())
}

/// Dispatches until `done` holds. Returns false when `stop` was raised first.
fn pump(
    queue: &mut EventQueue<State>,
    state: &mut State,
    stop: &AtomicBool,
    done: impl Fn(&State) -> bool,
) -> anyhow::Result<bool> {
    loop {
        queue.dispatch_pending(state)?;
        if done(state) {
            return Ok(true);
        }
        if stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        queue.flush()?;
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut poll = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd for the connection's open descriptor.
        let ready = unsafe { libc::poll(&mut poll, 1, POLL_MS) };
        if ready > 0 {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
        } else if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        }
    }
}

/// A client shm buffer, mapped for reading.
struct ShmBuffer {
    layout: (u32, u32, u32, u32),
    _file: File,
    map: *mut libc::c_void,
    len: usize,
    pool: WlShmPool,
    buffer: WlBuffer,
}

impl ShmBuffer {
    fn new(shm: &WlShm, qh: &QueueHandle<State>, layout: (u32, u32, u32, u32)) -> anyhow::Result<Self> {
        let (format, width, height, stride) = layout;
        let len = stride as usize * height as usize;
        // SAFETY: a fresh anonymous file; the descriptor is owned by `file`.
        let fd = unsafe { libc::memfd_create(c"portald-screencast".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: `fd` was just created and is owned by nothing else.
        let file = unsafe { File::from_raw_fd(fd) };
        file.set_len(len as u64)?;
        // SAFETY: maps `len` bytes of the file we sized above, read-only.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error().into());
        }
        let pool = shm.create_pool(file.as_fd(), len as i32, qh, ());
        let format = wl_shm::Format::try_from(format)
            .map_err(|_| anyhow::anyhow!("unknown shm format {format:#x}"))?;
        let buffer = pool.create_buffer(0, width as i32, height as i32, stride as i32, format, qh, ());
        Ok(Self {
            layout,
            _file: file,
            map,
            len,
            pool,
            buffer,
        })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping lives as long as self and is `len` bytes.
        unsafe { std::slice::from_raw_parts(self.map.cast::<u8>(), self.len) }
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
        // SAFETY: unmaps exactly the region mapped in `new`.
        unsafe { libc::munmap(self.map, self.len) };
    }
}

/// Reuses a frame no consumer still holds, or makes a new one.
fn recycle(pool: &mut Vec<Arc<Frame>>, width: u32, height: u32, stride: u32) -> Arc<Frame> {
    let len = stride as usize * height as usize;
    if let Some(index) = pool.iter().position(|frame| Arc::strong_count(frame) == 1) {
        let mut frame = pool.swap_remove(index);
        if let Some(inner) = Arc::get_mut(&mut frame) {
            inner.width = width;
            inner.height = height;
            inner.stride = stride;
            inner.data.resize(len, 0);
            return frame;
        }
    }
    Arc::new(Frame {
        width,
        height,
        stride,
        data: vec![0; len],
    })
}

/// Captures `output_name` until `stop` is raised or capture fails, handing
/// each frame to `sink`. Frames come no faster than `max_fps`. Every frame is
/// `size` (the mode the stream was offered at) with tight rows; a mode change
/// ends the capture, since the stream cannot follow it.
pub fn run(
    output_name: &str,
    size: (u32, u32),
    overlay_cursor: bool,
    max_fps: u32,
    stop: &AtomicBool,
    mut sink: impl FnMut(Arc<Frame>),
) -> anyhow::Result<()> {
    let (mut queue, mut state, globals) = connect()?;
    let qh = queue.handle();
    let output = state
        .outputs
        .iter()
        .find(|output| output.info.name == output_name)
        .map(|output| output.proxy.clone())
        .ok_or_else(|| anyhow::anyhow!("output {output_name} is gone"))?;
    let shm: WlShm = globals.bind(&qh, 1..=1, ())?;
    let manager: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 3..=3, ())
        .map_err(|error| anyhow::anyhow!("compd offers no zwlr_screencopy_manager_v1 v3: {error}"))?;
    let interval = Duration::from_secs(1) / max_fps.max(1);
    let mut shm_buffer: Option<ShmBuffer> = None;
    let mut frames: Vec<Arc<Frame>> = Vec::new();
    let mut failures = 0;
    let mut first = true;

    while !stop.load(Ordering::Relaxed) {
        let asked = Instant::now();
        state.copy = Copy::default();
        let frame = manager.capture_output(i32::from(overlay_cursor), &output, &qh, ());
        if !pump(&mut queue, &mut state, stop, |s| s.copy.buffer_done || s.copy.failed)? {
            frame.destroy();
            break;
        }
        let Some(layout) = state.copy.shm.filter(|_| !state.copy.failed) else {
            frame.destroy();
            anyhow::bail!("compd offered no shm layout for {output_name}");
        };
        let (format, width, height, stride) = layout;
        if (width, height) != size {
            frame.destroy();
            anyhow::bail!(
                "{output_name} changed mode from {}x{} to {width}x{height}",
                size.0,
                size.1
            );
        }
        if stride < width * 4 {
            frame.destroy();
            anyhow::bail!("screencopy stride {stride} is too small for width {width}");
        }
        if format != wl_shm::Format::Xrgb8888 as u32 && format != wl_shm::Format::Argb8888 as u32 {
            frame.destroy();
            anyhow::bail!("unsupported screencopy format {format:#x}");
        }
        if shm_buffer.as_ref().map(|b| b.layout) != Some(layout) {
            shm_buffer = Some(ShmBuffer::new(&shm, &qh, layout)?);
        }
        let buffer = shm_buffer.as_ref().expect("buffer made above");
        // The first copy is answered at once; later ones wait for damage.
        if first {
            frame.copy(&buffer.buffer);
        } else {
            frame.copy_with_damage(&buffer.buffer);
        }
        if !pump(&mut queue, &mut state, stop, |s| s.copy.ready || s.copy.failed)? {
            frame.destroy();
            break;
        }
        frame.destroy();
        if state.copy.failed {
            failures += 1;
            if failures >= MAX_FAILURES {
                anyhow::bail!("{failures} screencopy failures in a row on {output_name}");
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        failures = 0;
        first = false;

        // Rows are repacked tight (width * 4): the stream was offered that
        // stride, whatever padding compd puts in its shm layout.
        let tight = width * 4;
        let mut picture = recycle(&mut frames, width, height, tight);
        let target = &mut Arc::get_mut(&mut picture).expect("unshared frame").data;
        let source = buffer.bytes();
        if !state.copy.y_invert && stride == tight {
            target.copy_from_slice(source);
        } else {
            let (row, pitch) = (tight as usize, stride as usize);
            for (y, line) in target.chunks_exact_mut(row).enumerate() {
                let from = if state.copy.y_invert {
                    (height as usize - 1 - y) * pitch
                } else {
                    y * pitch
                };
                line.copy_from_slice(&source[from..from + row]);
            }
        }
        frames.push(picture.clone());
        frames.truncate(4);
        sink(picture);

        if let Some(wait) = interval.checked_sub(asked.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    Ok(())
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

impl Dispatch<WlOutput, usize> for State {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.get_mut(*index) else {
            return;
        };
        match event {
            wl_output::Event::Geometry { x, y, .. } if !output.logical => {
                output.info.x = x;
                output.info.y = y;
            }
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                ..
            } if flags.contains(wl_output::Mode::Current) => {
                output.info.mode_width = width;
                output.info.mode_height = height;
            }
            wl_output::Event::Name { name } => output.info.name = name,
            wl_output::Event::Description { description } => output.info.description = description,
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.get_mut(*index) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                output.info.x = x;
                output.info.y = y;
                output.logical = true;
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                output.info.width = width;
                output.info.height = height;
                output.logical = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let copy = &mut state.copy;
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let format = match format {
                    WEnum::Value(format) => format as u32,
                    WEnum::Unknown(raw) => raw,
                };
                copy.shm = Some((format, width, height, stride));
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => copy.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                copy.y_invert = matches!(flags, WEnum::Value(f) if f.contains(zwlr_screencopy_frame_v1::Flags::YInvert));
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => copy.ready = true,
            zwlr_screencopy_frame_v1::Event::Failed => copy.failed = true,
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(State: ignore WlShm);
wayland_client::delegate_noop!(State: ignore WlShmPool);
wayland_client::delegate_noop!(State: ignore WlBuffer);
wayland_client::delegate_noop!(State: ZxdgOutputManagerV1);
wayland_client::delegate_noop!(State: ZwlrScreencopyManagerV1);
