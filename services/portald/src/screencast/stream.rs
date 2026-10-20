// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The consumer side of a cast: one PipeWire `Video/Source` node per session,
//! fed by a capture thread. The node drives its own graph: each captured frame
//! triggers one process cycle, which copies the newest frame into a PipeWire
//! buffer. Dropping the [`Handle`] stops both threads.
use crate::screencast::capture::{self, Frame, OutputInfo};
use pipewire as pw;
use pw::spa;
use pw::spa::pod::{ChoiceValue, Object, Pod, Property, Value};
use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Rectangle};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long `Start` waits for PipeWire to publish the node.
const READY_TIMEOUT: Duration = Duration::from_secs(5);
/// A static screen sends no frames; consumers still need one now and then
/// (a newly attached one needs its first). The newest frame is re-sent after
/// this long without a fresh one.
const KEEPALIVE: Duration = Duration::from_secs(1);

enum Message {
    Frame(Arc<Frame>),
    /// The capture thread ended; the cast is over.
    CaptureEnded,
    Quit,
}

/// A running cast. Dropping it stops capture and removes the node.
pub struct Handle {
    pub node_id: u32,
    stop: Arc<AtomicBool>,
    quit: pw::channel::Sender<Message>,
    thread: Option<JoinHandle<()>>,
    ended: Arc<AtomicBool>,
}

impl Handle {
    /// Whether the cast ended on its own (output gone, capture failure).
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.quit.send(Message::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts casting `output`. Blocks until the node has an id, so call it off
/// the async runtime.
pub fn start(output: OutputInfo, overlay_cursor: bool, max_fps: u32) -> anyhow::Result<Handle> {
    let (sender, receiver) = pw::channel::channel::<Message>();
    let (ready, node) = mpsc::channel::<anyhow::Result<u32>>();
    let stop = Arc::new(AtomicBool::new(false));
    let ended = Arc::new(AtomicBool::new(false));
    let thread = {
        let sender = sender.clone();
        let stop = stop.clone();
        let ended = ended.clone();
        std::thread::Builder::new()
            .name("portald-cast".into())
            .spawn(move || {
                let failure = ready.clone();
                if let Err(error) = serve(&output, overlay_cursor, max_fps, receiver, sender, &stop, ready) {
                    tracing::warn!(%error, output = %output.name, "screencast stream ended");
                    let _ = failure.send(Err(error));
                }
                stop.store(true, Ordering::Relaxed);
                ended.store(true, Ordering::Relaxed);
            })?
    };
    let mut handle = Handle {
        node_id: 0,
        stop,
        quit: sender,
        thread: Some(thread),
        ended,
    };
    match node.recv_timeout(READY_TIMEOUT) {
        Ok(Ok(node_id)) => {
            handle.node_id = node_id;
            Ok(handle)
        }
        Ok(Err(error)) => Err(error),
        Err(_) => anyhow::bail!("PipeWire did not publish the screencast node in time"),
    }
}

fn serialise(object: Object) -> anyhow::Result<Vec<u8>> {
    Ok(spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(object),
    )
    .map_err(|error| anyhow::anyhow!("pod serialisation: {error:?}"))?
    .0
    .into_inner())
}

/// The one format offered: BGRx at the output's mode size, variable rate up
/// to `max_fps`.
fn format(width: u32, height: u32, max_fps: u32) -> anyhow::Result<Vec<u8>> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::video::VideoFormat;
    serialise(spa::pod::object! {
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(FormatProperties::VideoFormat, Id, VideoFormat::BGRx),
        spa::pod::property!(FormatProperties::VideoSize, Rectangle, Rectangle { width, height }),
        spa::pod::property!(FormatProperties::VideoFramerate, Fraction, Fraction { num: 0, denom: 1 }),
        spa::pod::property!(
            FormatProperties::VideoMaxFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: max_fps, denom: 1 },
            Fraction { num: 1, denom: 1 },
            Fraction { num: max_fps, denom: 1 }
        ),
    })
}

/// Buffers for the negotiated format: shm (memfd), one block of a frame.
fn buffers(stride: u32, height: u32) -> anyhow::Result<Vec<u8>> {
    let size = i32::try_from(u64::from(stride) * u64::from(height))?;
    let memfd = 1i32 << spa::sys::SPA_DATA_MemFd;
    serialise(Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_buffers,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: 4,
                        min: 2,
                        max: 8,
                    },
                ))),
            ),
            Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
            Property::new(spa::sys::SPA_PARAM_BUFFERS_size, Value::Int(size)),
            Property::new(spa::sys::SPA_PARAM_BUFFERS_stride, Value::Int(stride as i32)),
            Property::new(
                spa::sys::SPA_PARAM_BUFFERS_dataType,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags {
                        default: memfd,
                        flags: vec![memfd],
                    },
                ))),
            ),
        ],
    })
}

struct Data {
    latest: Rc<RefCell<Option<Arc<Frame>>>>,
    sent: Rc<Cell<Option<Instant>>>,
    ready: Option<mpsc::Sender<anyhow::Result<u32>>>,
    stride: u32,
    height: u32,
}

fn serve(
    output: &OutputInfo,
    overlay_cursor: bool,
    max_fps: u32,
    receiver: pw::channel::Receiver<Message>,
    sender: pw::channel::Sender<Message>,
    stop: &Arc<AtomicBool>,
    ready: mpsc::Sender<anyhow::Result<u32>>,
) -> anyhow::Result<()> {
    let width = u32::try_from(output.mode_width)?;
    let height = u32::try_from(output.mode_height)?;
    if width == 0 || height == 0 {
        anyhow::bail!("output {} has no current mode", output.name);
    }
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let description = if output.description.is_empty() {
        output.name.clone()
    } else {
        output.description.clone()
    };
    let stream = pw::stream::StreamRc::new(
        core,
        "portald-screencast",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
            *pw::keys::NODE_DESCRIPTION => description.as_str(),
        },
    )?;

    let latest: Rc<RefCell<Option<Arc<Frame>>>> = Rc::new(RefCell::new(None));
    let sent: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let _listener = stream
        .add_local_listener_with_user_data(Data {
            latest: latest.clone(),
            sent: sent.clone(),
            ready: Some(ready),
            stride: width * 4,
            height,
        })
        .state_changed(|stream, data, old, new| {
            tracing::info!(?old, ?new, node = stream.node_id(), "screencast stream state");
            // A consumer just attached: give it the newest frame now rather
            // than waiting for the screen to change.
            if matches!(new, pw::stream::StreamState::Streaming)
                && data.latest.borrow().is_some()
                && let Err(error) = stream.trigger_process()
            {
                tracing::debug!(%error, "process not triggered");
            }
            if matches!(new, pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming)
                && let Some(ready) = data.ready.take()
            {
                let _ = ready.send(Ok(stream.node_id()));
            }
            if let pw::stream::StreamState::Error(error) = new {
                tracing::warn!(%error, "screencast stream error");
            }
        })
        .param_changed(|stream, data, id, param| {
            if id != spa::param::ParamType::Format.as_raw() || param.is_none() {
                return;
            }
            match buffers(data.stride, data.height) {
                Ok(bytes) => {
                    if let Some(pod) = Pod::from_bytes(&bytes)
                        && let Err(error) = stream.update_params(&mut [pod])
                    {
                        tracing::warn!(%error, "buffer parameters refused");
                    }
                }
                Err(error) => tracing::warn!(%error, "buffer parameters not built"),
            }
        })
        .process(|stream, data| {
            let Some(frame) = data.latest.borrow().clone() else {
                return;
            };
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(block) = datas.first_mut() else {
                return;
            };
            let mut written = 0;
            if let Some(slice) = block.data() {
                written = slice.len().min(frame.data.len());
                slice[..written].copy_from_slice(&frame.data[..written]);
            }
            let chunk = block.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = frame.stride as i32;
            *chunk.size_mut() = written as u32;
            data.sent.set(Some(Instant::now()));
        })
        .register()?;

    let offer = format(width, height, max_fps)?;
    let mut params = [Pod::from_bytes(&offer).ok_or_else(|| anyhow::anyhow!("bad format pod"))?];
    stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::DRIVER
            | pw::stream::StreamFlags::ALLOC_BUFFERS
            | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    let capture = {
        let name = output.name.clone();
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("portald-capture".into())
            .spawn(move || {
                let frames = sender.clone();
                if let Err(error) = capture::run(&name, overlay_cursor, max_fps, &stop, |frame| {
                    let _ = frames.send(Message::Frame(frame));
                }) {
                    tracing::warn!(%error, output = %name, "screencast capture ended");
                }
                let _ = sender.send(Message::CaptureEnded);
            })?
    };

    let _attached = receiver.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        let stream = stream.clone();
        let latest = latest.clone();
        let first_frame = Cell::new(true);
        move |message| match message {
            Message::Frame(frame) => {
                if first_frame.replace(false) {
                    tracing::info!(width = frame.width, height = frame.height, "screencast first frame captured");
                }
                *latest.borrow_mut() = Some(frame);
                if matches!(stream.state(), pw::stream::StreamState::Streaming)
                    && let Err(error) = stream.trigger_process()
                {
                    tracing::debug!(%error, "process not triggered");
                }
            }
            Message::CaptureEnded | Message::Quit => mainloop.quit(),
        }
    });

    let keepalive = mainloop.loop_().add_timer({
        let stream = stream.clone();
        let latest = latest.clone();
        let sent = sent.clone();
        move |_| {
            let stale = sent.get().is_none_or(|at| at.elapsed() >= KEEPALIVE);
            if stale
                && latest.borrow().is_some()
                && matches!(stream.state(), pw::stream::StreamState::Streaming)
                && let Err(error) = stream.trigger_process()
            {
                tracing::debug!(%error, "keepalive not triggered");
            }
        }
    });
    let _ = keepalive.update_timer(Some(KEEPALIVE), Some(KEEPALIVE)).into_sync_result();

    mainloop.run();
    stop.store(true, Ordering::Relaxed);
    let _ = stream.disconnect();
    let _ = capture.join();
    Ok(())
}
