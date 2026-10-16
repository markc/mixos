// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Wayland clipboard and primary selection, on a worker thread of their
//! own (the design follows smithay-clipboard, MIT).
//!
//! The worker has its own event queue on the window's Wayland connection: a
//! data device and a primary-selection device per seat, and keyboard and
//! pointer listeners for the focus and the input serial a selection must
//! be set with. The window only sends it [`Request`]s and receives
//! [`Answer`]s; nothing here ever runs on, or blocks, the window's thread.
//!
//! A read is bounded three ways: one at a time ([`ReadError::Busy`]), at
//! most [`LIMIT`] bytes, counted as they arrive ([`ReadError::TooLarge`],
//! the transfer dropped at once), and at most [`READ_TIMEOUT`] long
//! ([`ReadError::TimedOut`]), so a source that keeps its pipe open forever
//! costs one closed descriptor, not a wedged clipboard.

use sctk::data_device_manager::data_device::{DataDevice, DataDeviceHandler};
use sctk::data_device_manager::data_offer::{DataOfferError, DataOfferHandler, DragOffer};
use sctk::data_device_manager::data_source::{CopyPasteSource, DataSourceHandler};
use sctk::data_device_manager::{DataDeviceManagerState, ReadPipe, WritePipe};
use sctk::primary_selection::PrimarySelectionManagerState;
use sctk::primary_selection::device::{PrimarySelectionDevice, PrimarySelectionDeviceHandler};
use sctk::primary_selection::selection::{PrimarySelectionSource, PrimarySelectionSourceHandler};
use sctk::reexports::calloop::timer::{TimeoutAction, Timer};
use sctk::reexports::calloop::{self, EventLoop, LoopHandle, PostAction, RegistrationToken};
use sctk::reexports::calloop_wayland_source::WaylandSource;
use sctk::reexports::client::globals::registry_queue_init;
use sctk::reexports::client::protocol::wl_data_device::WlDataDevice;
use sctk::reexports::client::protocol::wl_data_device_manager::DndAction;
use sctk::reexports::client::protocol::wl_data_source::WlDataSource;
use sctk::reexports::client::protocol::wl_keyboard::{self, WlKeyboard};
use sctk::reexports::client::protocol::wl_pointer::WlPointer;
use sctk::reexports::client::protocol::wl_seat::WlSeat;
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use sctk::reexports::client::{Connection, Dispatch, Proxy, QueueHandle, backend::ObjectId};
use sctk::reexports::protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
    zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
};
use sctk::registry::{ProvidesRegistryState, RegistryState};
use sctk::seat::pointer::{PointerData, PointerEvent, PointerEventKind, PointerHandler};
use sctk::seat::{Capability, SeatHandler, SeatState};
use sctk::{
    delegate_data_device, delegate_pointer, delegate_primary_selection, delegate_registry,
    delegate_seat, registry_handlers,
};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// The largest paste read (the terminal's own paste limit).
pub const LIMIT: usize = 16 * 1024 * 1024;

/// How long a read may take before it is dropped.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Text types, best first.
const MIME_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

/// What the window asks of the worker.
#[derive(Debug)]
pub enum Request {
    /// Own the clipboard (or the primary selection) with `text`.
    Write {
        text: String,
        primary: bool,
    },
    /// Read it; [`Answer`] `id` follows.
    Read {
        id: u64,
        primary: bool,
    },
    Exit,
}

/// Why a read gave no text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Another read is still in progress.
    Busy,
    /// More than [`LIMIT`] bytes.
    TooLarge,
    /// No answer within [`READ_TIMEOUT`].
    TimedOut,
    /// Nothing to read: no selection, no text type, no focus.
    Empty,
    Failed(String),
}

/// The outcome of read `id`.
#[derive(Debug)]
pub struct Answer {
    pub id: u64,
    pub result: Result<String, ReadError>,
}

/// Start the worker on the Wayland display `display`; requests go to the
/// returned sender, answers to `answers` (then `wake` runs).
///
/// # Safety
/// `display` must be a valid `wl_display` that outlives the worker (the
/// worker exits on [`Request::Exit`] or when the sender is dropped).
pub unsafe fn spawn(
    display: *mut std::ffi::c_void,
    answers: Sender<Answer>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> Option<calloop::channel::Sender<Request>> {
    // SAFETY: the caller guarantees the display; the backend shares it.
    let backend = unsafe { wayland_backend::client::Backend::from_foreign_display(display.cast()) };
    let connection = Connection::from_backend(backend);
    let (tx, rx) = calloop::channel::channel();
    let spawned = std::thread::Builder::new()
        .name("term-selection".into())
        .spawn(move || run(connection, rx, answers, wake));
    match spawned {
        Ok(_) => Some(tx),
        Err(error) => {
            eprintln!("term: clipboard worker: {error}");
            None
        }
    }
}

fn run(
    connection: Connection,
    requests: calloop::channel::Channel<Request>,
    answers: Sender<Answer>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let Ok((globals, queue)) = registry_queue_init::<Selection>(&connection) else {
        eprintln!("term: clipboard worker: no Wayland registry");
        return;
    };
    let Ok(mut event_loop) = EventLoop::<Selection>::try_new() else {
        return;
    };
    let qh = queue.handle();
    let mut state = Selection {
        registry: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        clipboard: DataDeviceManagerState::bind(&globals, &qh).ok(),
        primary: PrimarySelectionManagerState::bind(&globals, &qh).ok(),
        seats: HashMap::new(),
        latest: None,
        qh,
        handle: event_loop.handle(),
        clipboard_text: Rc::from([]),
        primary_text: Rc::from([]),
        clipboard_sources: Vec::new(),
        primary_sources: Vec::new(),
        reads: Reads::new(answers, wake),
        exit: false,
    };
    for seat in state.seat_state.seats() {
        state.seats.insert(seat.id(), SeatData::default());
    }
    let handle = event_loop.handle();
    let inserted = handle.insert_source(requests, |event, _, state: &mut Selection| match event {
        calloop::channel::Event::Msg(Request::Write { text, primary }) => {
            state.write(text, primary);
        }
        calloop::channel::Event::Msg(Request::Read { id, primary }) => state.read(id, primary),
        calloop::channel::Event::Msg(Request::Exit) | calloop::channel::Event::Closed => {
            state.exit = true;
        }
    });
    if inserted.is_err()
        || WaylandSource::new(connection, queue)
            .insert(handle)
            .is_err()
    {
        return;
    }
    while !state.exit {
        if event_loop.dispatch(None, &mut state).is_err() {
            break;
        }
    }
}

/// The read in progress, if any, and where answers go. Generic over the
/// loop's state so the bounds are tested without a compositor.
pub struct Reads {
    current: Option<Current>,
    answers: Sender<Answer>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Removes a read's timer once the read ended on its own.
    cancel: Option<Box<dyn Fn(RegistrationToken)>>,
}

struct Current {
    id: u64,
    source: RegistrationToken,
    timer: RegistrationToken,
}

/// A loop state that owns a [`Reads`].
pub trait HasReads: 'static {
    fn reads(&mut self) -> &mut Reads;
}

impl Reads {
    pub fn new(answers: Sender<Answer>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            current: None,
            answers,
            wake,
            cancel: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.current.is_some()
    }

    pub fn answer(&self, id: u64, result: Result<String, ReadError>) {
        let _ = self.answers.send(Answer { id, result });
        (self.wake)();
    }

    /// Read `pipe` as read `id`: bounded by [`LIMIT`] and `timeout`.
    pub fn start<S: HasReads>(
        handle: &LoopHandle<'static, S>,
        state: &mut S,
        id: u64,
        pipe: ReadPipe,
        timeout: Duration,
    ) {
        if state.reads().busy() {
            state.reads().answer(id, Err(ReadError::Busy));
            return;
        }
        if let Err(error) = set_non_blocking(pipe.as_raw_fd()) {
            state
                .reads()
                .answer(id, Err(ReadError::Failed(error.to_string())));
            return;
        }
        let mut content = Vec::new();
        let source = handle.insert_source(pipe, move |_, file, state: &mut S| {
            // SAFETY: the descriptor stays owned by the source; it is only
            // read here.
            let file = unsafe { file.get_mut() };
            match drain(file, &mut content, LIMIT) {
                Drain::More => PostAction::Continue,
                Drain::Done => {
                    let text = text(std::mem::take(&mut content));
                    state.reads().finish(id, Ok(text), true);
                    PostAction::Remove
                }
                Drain::TooLarge => {
                    state.reads().finish(id, Err(ReadError::TooLarge), true);
                    PostAction::Remove
                }
                Drain::Failed(error) => {
                    state
                        .reads()
                        .finish(id, Err(ReadError::Failed(error)), true);
                    PostAction::Remove
                }
            }
        });
        let Ok(source) = source else {
            state
                .reads()
                .answer(id, Err(ReadError::Failed("cannot watch the pipe".into())));
            return;
        };
        let timer_handle = handle.clone();
        let timer =
            handle.insert_source(Timer::from_duration(timeout), move |_, _, state: &mut S| {
                let reads = state.reads();
                let source = reads
                    .current
                    .as_ref()
                    .filter(|c| c.id == id)
                    .map(|c| c.source);
                if let Some(source) = source {
                    // Dropping the source closes the pipe: the source's end
                    // sees EPIPE and stops.
                    timer_handle.remove(source);
                    reads.finish(id, Err(ReadError::TimedOut), false);
                }
                TimeoutAction::Drop
            });
        let Ok(timer) = timer else {
            handle.remove(source);
            state
                .reads()
                .answer(id, Err(ReadError::Failed("cannot arm the timeout".into())));
            return;
        };
        state.reads().current = Some(Current { id, source, timer });
        let timers = handle.clone();
        state.reads().cancel = Some(Box::new(move |timer| timers.remove(timer)));
    }

    /// End read `id` with `result`. `from_source`: the read's own source is
    /// removing itself, and its timer is cancelled.
    fn finish(&mut self, id: u64, result: Result<String, ReadError>, from_source: bool) {
        let Some(current) = self.current.take_if(|c| c.id == id) else {
            return;
        };
        if from_source && let Some(cancel) = self.cancel.as_ref() {
            cancel(current.timer);
        }
        self.answer(id, result);
    }
}

/// How far a drain got.
#[derive(Debug, PartialEq, Eq)]
pub enum Drain {
    /// The pipe would block: wait for more.
    More,
    /// End of transfer.
    Done,
    /// The transfer passed the limit: drop it.
    TooLarge,
    Failed(String),
}

/// Read everything `reader` has now into `content`, never past `limit`.
pub fn drain(reader: &mut impl Read, content: &mut Vec<u8>, limit: usize) -> Drain {
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Drain::Done,
            Ok(n) => {
                if content.len() + n > limit {
                    content.clear();
                    content.shrink_to_fit();
                    return Drain::TooLarge;
                }
                content.extend_from_slice(&buffer[..n]);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Drain::More,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Drain::Failed(error.to_string()),
        }
    }
}

/// Transfer bytes as paste text: UTF-8 (invalid sequences replaced), line
/// endings as a paste expects them. Bounded by [`LIMIT`] already.
pub fn text(content: Vec<u8>) -> String {
    let text = String::from_utf8(content)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text
    }
}

fn set_non_blocking(fd: std::os::fd::RawFd) -> std::io::Result<()> {
    // SAFETY: fcntl on a descriptor this process owns.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// The first of our text types `offered` has.
fn text_type(offered: &[String]) -> Option<&'static str> {
    MIME_TYPES
        .into_iter()
        .find(|wanted| offered.iter().any(|o| o == wanted))
}

#[derive(Default)]
struct SeatData {
    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
    clipboard: Option<DataDevice>,
    primary: Option<PrimarySelectionDevice>,
    focused: bool,
    /// The latest input serial: a selection is set with it.
    serial: u32,
}

struct Selection {
    registry: RegistryState,
    seat_state: SeatState,
    clipboard: Option<DataDeviceManagerState>,
    primary: Option<PrimarySelectionManagerState>,
    seats: HashMap<ObjectId, SeatData>,
    /// The seat that saw input last.
    latest: Option<ObjectId>,
    qh: QueueHandle<Self>,
    handle: LoopHandle<'static, Self>,
    clipboard_text: Rc<[u8]>,
    primary_text: Rc<[u8]>,
    clipboard_sources: Vec<CopyPasteSource>,
    primary_sources: Vec<PrimarySelectionSource>,
    reads: Reads,
    exit: bool,
}

impl HasReads for Selection {
    fn reads(&mut self) -> &mut Reads {
        &mut self.reads
    }
}

impl Selection {
    fn seat(&self) -> Option<&SeatData> {
        self.seats.get(self.latest.as_ref()?).filter(|s| s.focused)
    }

    fn write(&mut self, text: String, primary: bool) {
        let Some(seat) = self.seat() else {
            return;
        };
        let serial = seat.serial;
        let content: Rc<[u8]> = Rc::from(text.into_bytes());
        if primary {
            let (Some(manager), Some(device)) = (&self.primary, seat.primary.as_ref()) else {
                return;
            };
            let source = manager.create_selection_source(&self.qh, MIME_TYPES);
            source.set_selection(device, serial);
            self.primary_text = content;
            self.primary_sources.push(source);
        } else {
            let (Some(manager), Some(device)) = (&self.clipboard, seat.clipboard.as_ref()) else {
                return;
            };
            let source = manager.create_copy_paste_source(&self.qh, MIME_TYPES);
            source.set_selection(device, serial);
            self.clipboard_text = content;
            self.clipboard_sources.push(source);
        }
    }

    fn receive(&self, primary: bool) -> Result<ReadPipe, ReadError> {
        let seat = self.seat().ok_or(ReadError::Empty)?;
        if primary {
            let offer = seat
                .primary
                .as_ref()
                .and_then(|d| d.data().selection_offer())
                .ok_or(ReadError::Empty)?;
            let mime = offer.with_mime_types(text_type).ok_or(ReadError::Empty)?;
            offer
                .receive(mime.to_owned())
                .map_err(|e| ReadError::Failed(e.to_string()))
        } else {
            let offer = seat
                .clipboard
                .as_ref()
                .and_then(|d| d.data().selection_offer())
                .ok_or(ReadError::Empty)?;
            let mime = offer.with_mime_types(text_type).ok_or(ReadError::Empty)?;
            offer.receive(mime.to_owned()).map_err(|e| match e {
                DataOfferError::InvalidReceive => ReadError::Empty,
                DataOfferError::Io(e) => ReadError::Failed(e.to_string()),
            })
        }
    }

    fn read(&mut self, id: u64, primary: bool) {
        if self.reads.busy() {
            self.reads.answer(id, Err(ReadError::Busy));
            return;
        }
        match self.receive(primary) {
            Ok(pipe) => {
                let handle = self.handle.clone();
                Reads::start(&handle, self, id, pipe, READ_TIMEOUT);
            }
            Err(error) => self.reads.answer(id, Err(error)),
        }
    }

    /// Serve a paste of our own selection, without blocking.
    fn send(&mut self, primary: bool, mime: &str, pipe: WritePipe) {
        if !MIME_TYPES.contains(&mime) || set_non_blocking(pipe.as_raw_fd()).is_err() {
            return;
        }
        let content = if primary {
            self.primary_text.clone()
        } else {
            self.clipboard_text.clone()
        };
        let mut written = 0;
        let _ = self.handle.insert_source(pipe, move |_, file, _| {
            // SAFETY: only written here; the source owns the descriptor.
            let file = unsafe { file.get_mut() };
            loop {
                match file.write(&content[written..]) {
                    Ok(n) => {
                        written += n;
                        if written == content.len() {
                            return PostAction::Remove;
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        return PostAction::Continue;
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(_) => return PostAction::Remove,
                }
            }
        });
    }
}

impl SeatHandler for Selection {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: WlSeat) {
        self.seats.insert(seat.id(), SeatData::default());
    }

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        let data = self.seats.entry(seat.id()).or_default();
        match capability {
            Capability::Keyboard => {
                data.keyboard = Some(seat.get_keyboard(qh, seat.id()));
                // Selections follow the keyboard.
                if data.clipboard.is_none() {
                    data.clipboard = self
                        .clipboard
                        .as_ref()
                        .map(|m| m.get_data_device(qh, &seat));
                }
                if data.primary.is_none() {
                    data.primary = self
                        .primary
                        .as_ref()
                        .map(|m| m.get_selection_device(qh, &seat));
                }
            }
            Capability::Pointer => {
                data.pointer = self.seat_state.get_pointer(qh, &seat).ok();
            }
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        let Some(data) = self.seats.get_mut(&seat.id()) else {
            return;
        };
        match capability {
            Capability::Keyboard => {
                data.clipboard = None;
                data.primary = None;
                if let Some(keyboard) = data.keyboard.take()
                    && keyboard.version() >= 3
                {
                    keyboard.release();
                }
            }
            Capability::Pointer => {
                if let Some(pointer) = data.pointer.take()
                    && pointer.version() >= 3
                {
                    pointer.release();
                }
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: WlSeat) {
        self.seats.remove(&seat.id());
    }
}

impl PointerHandler for Selection {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        let Some(seat) = pointer.data::<PointerData>().map(|d| d.seat().id()) else {
            return;
        };
        let Some(data) = self.seats.get_mut(&seat) else {
            return;
        };
        for event in events {
            if let PointerEventKind::Press { serial, .. }
            | PointerEventKind::Release { serial, .. } = event.kind
            {
                data.serial = serial;
                self.latest = Some(seat.clone());
            }
        }
    }
}

impl Dispatch<WlKeyboard, ObjectId> for Selection {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        seat: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(data) = state.seats.get_mut(seat) else {
            return;
        };
        match event {
            wl_keyboard::Event::Key { serial, .. }
            | wl_keyboard::Event::Modifiers { serial, .. } => {
                data.serial = serial;
                state.latest = Some(seat.clone());
            }
            wl_keyboard::Event::Enter { serial, .. } => {
                data.serial = serial;
                data.focused = true;
                state.latest = Some(seat.clone());
            }
            wl_keyboard::Event::Leave { .. } => {
                data.serial = 0;
                data.focused = false;
            }
            _ => {}
        }
    }
}

impl DataDeviceHandler for Selection {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataDevice,
        _: f64,
        _: f64,
        _: &WlSurface,
    ) {
    }
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}
    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
}

impl DataSourceHandler for Selection {
    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        mime: String,
        pipe: WritePipe,
    ) {
        self.send(false, &mime, pipe);
    }
    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        self.clipboard_sources.retain(|s| s.inner() != source);
    }
    fn accept_mime(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        _: Option<String>,
    ) {
    }
    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
}

impl DataOfferHandler for Selection {
    fn source_actions(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
    fn selected_action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
}

impl PrimarySelectionDeviceHandler for Selection {
    fn selection(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpPrimarySelectionDeviceV1,
    ) {
    }
}

impl PrimarySelectionSourceHandler for Selection {
    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpPrimarySelectionSourceV1,
        mime: String,
        pipe: WritePipe,
    ) {
        self.send(true, &mime, pipe);
    }
    fn cancelled(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        source: &ZwpPrimarySelectionSourceV1,
    ) {
        self.primary_sources.retain(|s| s.inner() != source);
    }
}

impl ProvidesRegistryState for Selection {
    registry_handlers![SeatState];

    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
}

delegate_seat!(Selection);
delegate_pointer!(Selection);
delegate_data_device!(Selection);
delegate_primary_selection!(Selection);
delegate_registry!(Selection);

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{FromRawFd, OwnedFd};

    /// A pipe: (read end, write end).
    fn pipe() -> (OwnedFd, std::fs::File) {
        let mut fds = [0; 2];
        // SAFETY: two valid out slots.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: both are new descriptors this test owns.
        unsafe {
            (
                OwnedFd::from_raw_fd(fds[0]),
                std::fs::File::from_raw_fd(fds[1]),
            )
        }
    }

    #[test]
    fn drain_stops_at_the_limit_and_keeps_nothing() {
        let mut content = Vec::new();
        let mut small: &[u8] = b"hello";
        assert_eq!(drain(&mut small, &mut content, 8), Drain::Done);
        assert_eq!(content, b"hello");
        let mut content = Vec::new();
        let mut big: &[u8] = &[b'x'; 9];
        assert_eq!(drain(&mut big, &mut content, 8), Drain::TooLarge);
        assert!(content.is_empty());
        assert_eq!(text(b"a\r\nb\rc".to_vec()), "a\nb\nc");
        assert_eq!(text(vec![b'a', 0xff, b'b']), "a\u{fffd}b");
    }

    struct Test {
        reads: Reads,
    }

    impl HasReads for Test {
        fn reads(&mut self) -> &mut Reads {
            &mut self.reads
        }
    }

    fn run_until(
        event_loop: &mut EventLoop<'static, Test>,
        state: &mut Test,
        answers: &std::sync::mpsc::Receiver<Answer>,
    ) -> Answer {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(answer) = answers.try_recv() {
                return answer;
            }
            assert!(std::time::Instant::now() < deadline, "no answer");
            event_loop
                .dispatch(Some(Duration::from_millis(20)), state)
                .unwrap();
        }
    }

    /// Review findings: a source that keeps its pipe open must not wedge
    /// the clipboard, a transfer past the limit is dropped while it
    /// arrives, and only one read is ever outstanding.
    #[test]
    fn reads_are_one_at_a_time_bounded_and_time_out() {
        let (tx, answers) = std::sync::mpsc::channel();
        let mut event_loop = EventLoop::<Test>::try_new().unwrap();
        let handle = event_loop.handle();
        let mut state = Test {
            reads: Reads::new(tx, Arc::new(|| {})),
        };

        // A source that never closes its end: the read times out.
        let (read, _held_open) = pipe();
        Reads::start(
            &handle,
            &mut state,
            1,
            ReadPipe::from(read),
            Duration::from_millis(100),
        );
        // A second read meanwhile is refused at once.
        let (other, _w) = pipe();
        Reads::start(
            &handle,
            &mut state,
            2,
            ReadPipe::from(other),
            Duration::from_secs(5),
        );
        let busy = run_until(&mut event_loop, &mut state, &answers);
        assert_eq!((busy.id, busy.result), (2, Err(ReadError::Busy)));
        let timed_out = run_until(&mut event_loop, &mut state, &answers);
        assert_eq!(
            (timed_out.id, timed_out.result),
            (1, Err(ReadError::TimedOut))
        );
        assert!(!state.reads.busy());

        // A transfer past the limit stops as soon as it passes it.
        let (read, mut write) = pipe();
        Reads::start(
            &handle,
            &mut state,
            3,
            ReadPipe::from(read),
            Duration::from_secs(5),
        );
        let writer = std::thread::spawn(move || {
            let chunk = vec![b'x'; 64 * 1024];
            let mut sent = 0;
            // The reader closes its end at the limit: the writer then fails.
            while sent <= LIMIT + chunk.len() {
                if write.write_all(&chunk).is_err() {
                    return sent;
                }
                sent += chunk.len();
            }
            sent
        });
        let too_large = run_until(&mut event_loop, &mut state, &answers);
        assert_eq!(
            (too_large.id, too_large.result),
            (3, Err(ReadError::TooLarge))
        );
        assert!(writer.join().unwrap() <= LIMIT + 64 * 1024);

        // An ordinary read.
        let (read, mut write) = pipe();
        write.write_all(b"pasted\r\n").unwrap();
        drop(write);
        Reads::start(
            &handle,
            &mut state,
            4,
            ReadPipe::from(read),
            Duration::from_secs(5),
        );
        let done = run_until(&mut event_loop, &mut state, &answers);
        assert_eq!((done.id, done.result), (4, Ok("pasted\n".into())));
        assert!(!state.reads.busy());
    }
}
