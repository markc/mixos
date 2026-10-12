// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Wayland clipboard and primary selection.
//!
//! egui writes the clipboard but knows no primary selection, and a
//! terminal needs both: a finished selection becomes PRIMARY, a middle
//! click pastes it. So the window keeps its own clipboard connection on the
//! window's display. Reads block until the compositor answers, so they run
//! on a worker thread and come back with a repaint.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// A clipboard read that finished: the pane it was for and its text.
pub type Pasted = (u64, Option<String>);

pub struct Clipboard {
    wayland: Option<Arc<Mutex<smithay_clipboard::Clipboard>>>,
    tx: Sender<Pasted>,
    rx: Receiver<Pasted>,
    wake: Arc<dyn Fn() + Send + Sync>,
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
        let wayland = match display {
            Some(wgpu::rwh::RawDisplayHandle::Wayland(handle)) => {
                // SAFETY: the caller keeps the display alive.
                let clipboard =
                    unsafe { smithay_clipboard::Clipboard::new(handle.display.as_ptr()) };
                Some(Arc::new(Mutex::new(clipboard)))
            }
            _ => None,
        };
        let (tx, rx) = channel();
        Self {
            wayland,
            tx,
            rx,
            wake,
        }
    }

    /// No platform clipboard: writes go through egui, reads answer nothing.
    pub fn none() -> Self {
        // SAFETY: no display.
        unsafe { Self::new(None, Arc::new(|| {})) }
    }

    /// Write the clipboard (through egui when there is no Wayland one) or
    /// the primary selection.
    pub fn write(&self, ctx: &egui::Context, text: String, primary: bool) {
        match (&self.wayland, primary) {
            (Some(wayland), true) => wayland.lock().expect("clipboard").store_primary(text),
            (Some(wayland), false) => wayland.lock().expect("clipboard").store(text),
            (None, true) => {}
            (None, false) => ctx.copy_text(text),
        }
    }

    /// Read the clipboard or the primary selection for `pane`; the text
    /// arrives through [`Clipboard::take`].
    pub fn read(&self, pane: u64, primary: bool) {
        let Some(wayland) = self.wayland.clone() else {
            let _ = self.tx.send((pane, None));
            (self.wake)();
            return;
        };
        let (tx, wake) = (self.tx.clone(), self.wake.clone());
        let spawned = std::thread::Builder::new()
            .name("term-clipboard".into())
            .spawn(move || {
                let clipboard = wayland.lock().expect("clipboard");
                let text = if primary {
                    clipboard.load_primary()
                } else {
                    clipboard.load()
                };
                drop(clipboard);
                // Line endings as a paste expects them.
                let _ = tx.send((pane, text.ok().map(|t| t.replace("\r\n", "\n"))));
                wake();
            });
        if let Err(error) = spawned {
            eprintln!("term: clipboard read: {error}");
        }
    }

    /// The reads that have finished, oldest first.
    pub fn take(&self) -> Vec<Pasted> {
        self.rx.try_iter().collect()
    }
}
