// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's side of the clipboard and primary selection.
//!
//! egui writes the clipboard but knows no primary selection, and a
//! terminal needs both: a finished selection becomes PRIMARY, a middle
//! click pastes it. The Wayland work runs on its own worker
//! ([`crate::selection`]); this side only sends requests and takes answers
//! over channels, so no clipboard transfer can ever block a frame. One read
//! is outstanding at a time: another while it runs is refused at once.

use crate::selection::{Answer, ReadError, Request};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// A finished read: the pane it was for and its text, or why there is none.
pub type Pasted = (u64, Result<String, ReadError>);

pub struct Clipboard {
    worker: Option<sctk::reexports::calloop::channel::Sender<Request>>,
    tx: Sender<Answer>,
    rx: Receiver<Answer>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// The read in flight: its id and the pane it pastes into.
    pending: Option<(u64, u64)>,
    next: u64,
    /// Reads answered here, without the worker.
    refused: Vec<Pasted>,
}

impl Clipboard {
    /// A clipboard on `display`, if it is a Wayland display.
    ///
    /// # Safety
    /// `display` must outlive the returned clipboard (eframe's window
    /// display does: the app is dropped before it).
    pub unsafe fn new(
        display: Option<wgpu::rwh::RawDisplayHandle>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (tx, rx) = channel();
        let worker = match display {
            Some(wgpu::rwh::RawDisplayHandle::Wayland(handle)) => {
                // SAFETY: the caller keeps the display alive.
                unsafe {
                    crate::selection::spawn(handle.display.as_ptr(), tx.clone(), wake.clone())
                }
            }
            _ => None,
        };
        Self {
            worker,
            tx,
            rx,
            wake,
            pending: None,
            next: 0,
            refused: Vec::new(),
        }
    }

    /// No platform clipboard: writes go through egui, reads answer nothing.
    pub fn none() -> Self {
        // SAFETY: no display.
        unsafe { Self::new(None, Arc::new(|| {})) }
    }

    /// A clipboard whose requests go to `worker` (tests).
    pub fn with_worker(worker: sctk::reexports::calloop::channel::Sender<Request>) -> Self {
        let mut clipboard = Self::none();
        clipboard.worker = Some(worker);
        clipboard
    }

    /// Write the clipboard (through egui when there is no Wayland worker)
    /// or the primary selection. Never blocks.
    pub fn write(&self, ctx: &egui::Context, text: String, primary: bool) {
        match &self.worker {
            Some(worker) => {
                let _ = worker.send(Request::Write { text, primary });
            }
            None if !primary => ctx.copy_text(text),
            None => {}
        }
    }

    /// Read the clipboard or the primary selection for `pane`; the text
    /// arrives through [`Clipboard::take`]. A read while one is in flight is
    /// answered [`ReadError::Busy`] at once.
    pub fn read(&mut self, pane: u64, primary: bool) {
        self.next += 1;
        let id = self.next;
        if self.pending.is_some() {
            self.refuse(pane, Err(ReadError::Busy));
            return;
        }
        let Some(worker) = &self.worker else {
            self.refuse(pane, Err(ReadError::Empty));
            return;
        };
        if worker.send(Request::Read { id, primary }).is_err() {
            self.refuse(
                pane,
                Err(ReadError::Failed("the clipboard worker stopped".into())),
            );
            return;
        }
        self.pending = Some((id, pane));
    }

    /// Answer a read for `pane` here, without the worker.
    fn refuse(&mut self, pane: u64, result: Result<String, ReadError>) {
        self.refused.push((pane, result));
        (self.wake)();
    }

    /// The reads that have finished, oldest first.
    pub fn take(&mut self) -> Vec<Pasted> {
        let mut out = std::mem::take(&mut self.refused);
        while let Ok(answer) = self.rx.try_recv() {
            match self.pending {
                Some((id, pane)) if id == answer.id => {
                    self.pending = None;
                    out.push((pane, answer.result));
                }
                // A stray answer (the worker refused a read the window
                // never counted): nothing to paste.
                _ => {}
            }
        }
        out
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            let _ = worker.send(Request::Exit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review finding: a read in flight must never make the window wait.
    /// A second read is refused at once, and a write still goes through.
    #[test]
    fn a_read_in_flight_never_blocks_the_window() {
        let (worker, requests) = sctk::reexports::calloop::channel::channel();
        let mut clipboard = Clipboard::with_worker(worker);
        clipboard.read(7, true);
        clipboard.read(8, false);
        assert_eq!(clipboard.take(), vec![(8, Err(ReadError::Busy))]);
        let ctx = egui::Context::default();
        clipboard.write(&ctx, "copied".into(), false);
        let sent: Vec<Request> = std::iter::from_fn(|| requests.try_recv().ok()).collect();
        assert!(matches!(
            &sent[..],
            [
                Request::Read {
                    id: 1,
                    primary: true
                },
                Request::Write { primary: false, .. }
            ]
        ));
        // The worker's answer arrives later and pastes into the first pane.
        clipboard
            .tx
            .send(Answer {
                id: 1,
                result: Ok("text".into()),
            })
            .unwrap();
        assert_eq!(clipboard.take(), vec![(7, Ok("text".into()))]);
        clipboard.read(9, false);
        assert!(clipboard.take().is_empty(), "a new read may start");
    }
}
