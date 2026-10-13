// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Showing file thumbnails: a cached thumbnail (a small PNG a thumbnailer
//! made, never the user's file) decoded under explicit limits on a
//! [`Decoder`] thread, ready to upload as a texture. Making thumbnails is
//! the headless `thumbs` library's part.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, mpsc};

/// The decoder's ceiling on memory (pixels, ICC and other chunks together):
/// an xx-large RGBA thumbnail needs 4 MiB.
const DECODE_MAX_ALLOC: u64 = 32 * 1024 * 1024;

/// Decode a thumbnail from the shared cache. The cache is shared with every
/// other program of the user, so the entry is read capped and only if it is
/// a regular file, its size is checked before decoding, and the decoder
/// runs under explicit limits. `None` when it is anything but a small,
/// whole PNG.
pub fn decode(path: &Path) -> Option<egui::ColorImage> {
    let bytes = thumbs::cache::read_regular(path, thumbs::cache::MAX_ENTRY).ok()?;
    let (w, h) = thumbs::png::size(&bytes)?;
    let max = thumbs::Size::XXLarge.px();
    if w > max || h > max {
        return None;
    }
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max);
    limits.max_image_height = Some(max);
    limits.max_alloc = Some(DECODE_MAX_ALLOC);
    reader.limits(limits);
    let image = reader.decode().ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        image.as_raw(),
    ))
}

/// A finished decode: the caller's key, and the image, or `None` when the
/// cache entry would not decode.
pub type Decoded<K> = (K, Option<egui::ColorImage>);

/// One request: its identity (unique per decoder), the caller's key, the
/// cache entry, and the context to repaint.
type Request<K> = (u64, K, PathBuf, egui::Context);

/// What the decoder thread waits on: the requests it will take next, in
/// order, and whether its owner is gone.
struct Inbox<K> {
    requests: VecDeque<Request<K>>,
    closed: bool,
}

struct Shared<K> {
    inbox: Mutex<Inbox<K>>,
    ready: Condvar,
}

/// One decoder thread, started on first use, serving requests in order.
/// A caller that shows one image at a time asks with [`Decoder::replace`]
/// (stepping through a folder never queues a decode per file); one that
/// shows many asks with [`Decoder::ask`]. Each finished decode repaints the
/// context it was asked with.
pub struct Decoder<K> {
    shared: Arc<Shared<K>>,
    tx: mpsc::Sender<(u64, Decoded<K>)>,
    rx: mpsc::Receiver<(u64, Decoded<K>)>,
    /// The thread's name.
    name: String,
    started: bool,
    /// The next request's identity.
    next: u64,
    /// Requests asked for and not yet taken, by key and identity: a decode
    /// answering anything else (another key, or a withdrawn request for
    /// the same key) is dropped when it arrives.
    asked: Vec<(K, u64)>,
    /// Decodes arrived and not yet taken.
    done: Vec<Decoded<K>>,
}

impl<K: Clone + PartialEq + Send + 'static> Decoder<K> {
    /// A decoder whose thread (started on the first request) is `name`.
    pub fn new(name: &str) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            shared: Arc::new(Shared {
                inbox: Mutex::new(Inbox {
                    requests: VecDeque::new(),
                    closed: false,
                }),
                ready: Condvar::new(),
            }),
            tx,
            rx,
            name: name.to_owned(),
            started: false,
            next: 0,
            asked: Vec::new(),
            done: Vec::new(),
        }
    }

    /// Whether `key` is outstanding: asked, or decoded and not yet taken.
    fn outstanding(&self, key: &K) -> bool {
        self.asked.iter().any(|(k, _)| k == key) || self.done.iter().any(|(k, _)| k == key)
    }

    /// Queue `key` (decoding the cache entry `path`) behind earlier
    /// requests. Already outstanding (asked, or decoded and not yet taken):
    /// nothing. `false` when the thread cannot be started.
    pub fn ask(&mut self, key: &K, path: &Path, ctx: &egui::Context) -> bool {
        if self.outstanding(key) {
            return true;
        }
        if !self.start() {
            return false;
        }
        let id = self.next;
        self.next += 1;
        self.asked.push((key.clone(), id));
        let mut inbox = self.shared.inbox.lock().unwrap_or_else(|e| e.into_inner());
        inbox
            .requests
            .push_back((id, key.clone(), path.to_owned(), ctx.clone()));
        drop(inbox);
        self.shared.ready.notify_one();
        true
    }

    /// Ask for `key` alone: every other key's request or unclaimed decode
    /// is withdrawn; `key`'s own, asked or done, is kept.
    pub fn replace(&mut self, key: &K, path: &Path, ctx: &egui::Context) -> bool {
        self.asked.retain(|(k, _)| k == key);
        self.done.retain(|(k, _)| k == key);
        let mut inbox = self.shared.inbox.lock().unwrap_or_else(|e| e.into_inner());
        inbox.requests.retain(|(_, k, _, _)| k == key);
        drop(inbox);
        self.ask(key, path, ctx)
    }

    /// Withdraw every request not yet taken, and drop unclaimed decodes. A
    /// withdrawn request's decode still in flight is dropped on arrival,
    /// even if its key is asked again.
    pub fn withdraw_all(&mut self) {
        self.asked.clear();
        self.done.clear();
        let mut inbox = self.shared.inbox.lock().unwrap_or_else(|e| e.into_inner());
        inbox.requests.clear();
    }

    /// The decode of `key`, once it is done.
    pub fn take(&mut self, key: &K) -> Option<Option<egui::ColorImage>> {
        self.collect();
        let at = self.done.iter().position(|(k, _)| k == key)?;
        Some(self.done.swap_remove(at).1)
    }

    /// Every decode done so far.
    pub fn drain(&mut self) -> Vec<Decoded<K>> {
        self.collect();
        std::mem::take(&mut self.done)
    }

    /// Whether anything asked is not yet taken.
    pub fn pending(&self) -> bool {
        !self.asked.is_empty() || !self.done.is_empty()
    }

    /// Move arrived decodes into `done`: only answers to a request still
    /// asked, which is then answered (no longer asked).
    fn collect(&mut self) {
        while let Ok((id, decoded)) = self.rx.try_recv() {
            if let Some(at) = self
                .asked
                .iter()
                .position(|(k, asked)| *asked == id && *k == decoded.0)
            {
                self.asked.swap_remove(at);
                self.done.push(decoded);
            }
        }
    }

    fn start(&mut self) -> bool {
        if self.started {
            return true;
        }
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name(self.name.clone())
            .spawn(move || decode_requests(&shared, &tx));
        self.started = spawned.is_ok();
        self.started
    }
}

impl<K> Drop for Decoder<K> {
    fn drop(&mut self) {
        let mut inbox = self.shared.inbox.lock().unwrap_or_else(|e| e.into_inner());
        inbox.closed = true;
        inbox.requests.clear();
        drop(inbox);
        self.shared.ready.notify_all();
    }
}

/// The decoder thread: take the next request, decode it, reply, repaint;
/// stop when the owner is gone.
fn decode_requests<K>(shared: &Shared<K>, tx: &mpsc::Sender<(u64, Decoded<K>)>) {
    loop {
        let (id, key, path, ctx) = {
            let mut inbox = shared.inbox.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if inbox.closed {
                    return;
                }
                if let Some(request) = inbox.requests.pop_front() {
                    break request;
                }
                inbox = shared.ready.wait(inbox).unwrap_or_else(|e| e.into_inner());
            }
        };
        let image = decode(&path);
        if tx.send((id, (key, image))).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A PNG signature and IHDR claiming `w`x`h`, with no image data.
    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        out.extend(13u32.to_be_bytes());
        out.extend(b"IHDR");
        out.extend(w.to_be_bytes());
        out.extend(h.to_be_bytes());
        out.extend([8, 6, 0, 0, 0]);
        out.extend([0; 4]);
        out.extend(0u32.to_be_bytes());
        out.extend(b"IEND");
        out.extend([0; 4]);
        out
    }

    #[test]
    fn decode_takes_only_small_regular_pngs() {
        let dir = tempfile::tempdir().unwrap();
        let blank = thumbs::png::blank();
        let good = dir.path().join("good.png");
        std::fs::write(&good, &blank).unwrap();
        let image = decode(&good).expect("a valid PNG decodes");
        assert_eq!(image.size, [1, 1]);

        let truncated = dir.path().join("truncated.png");
        std::fs::write(&truncated, &blank[..blank.len() / 2]).unwrap();
        assert!(decode(&truncated).is_none());
        let garbage = dir.path().join("garbage.png");
        std::fs::write(&garbage, b"not a png at all").unwrap();
        assert!(decode(&garbage).is_none());

        // Refused from its header: never handed to the decoder.
        let huge = dir.path().join("huge.png");
        std::fs::write(&huge, png_header(4096, 16)).unwrap();
        assert!(decode(&huge).is_none());

        let link = dir.path().join("link.png");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        assert!(decode(&link).is_none(), "never through a symlink");
        assert!(decode(dir.path()).is_none());
    }

    fn wait<T>(mut poll: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = poll() {
                return value;
            }
            assert!(Instant::now() < deadline, "no answer");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Replacing answers the latest request; one thread serves them all.
    #[test]
    fn replace_answers_the_latest_request() {
        let dir = tempfile::tempdir().unwrap();
        let cached = dir.path().join("cached.png");
        std::fs::write(&cached, thumbs::png::blank()).unwrap();
        let ctx = egui::Context::default();
        let mut decoder = Decoder::new("test-decoder");
        assert!(decoder.replace(&"a", &cached, &ctx));
        assert!(decoder.replace(&"b", &cached, &ctx));
        assert!(decoder.replace(&"b", &cached, &ctx), "asked once");
        let decoded = wait(|| decoder.take(&"b"));
        assert!(decoded.is_some());
        assert!(!decoder.pending());
        assert!(decoder.take(&"a").is_none(), "withdrawn");
    }

    /// A decode of a withdrawn request never answers a later request for the
    /// same key: the later one's own answer does.
    #[test]
    fn a_withdrawn_decode_never_answers_a_later_request() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.png");
        std::fs::write(&good, thumbs::png::blank()).unwrap();
        let broken = dir.path().join("broken.png");
        std::fs::write(&broken, b"no").unwrap();
        let ctx = egui::Context::default();
        let mut decoder = Decoder::new("test-decoder");
        assert!(decoder.ask(&"k", &good, &ctx));
        // Decoded and sent, not yet taken.
        std::thread::sleep(Duration::from_millis(300));
        decoder.withdraw_all();
        assert!(decoder.ask(&"k", &broken, &ctx));
        let decoded = wait(|| decoder.take(&"k"));
        assert!(decoded.is_none(), "the broken entry's own answer");
        assert!(!decoder.pending());
    }

    /// A decode done but not yet taken still counts: asking or replacing
    /// its key again neither decodes it twice nor drops it.
    #[test]
    fn a_done_decode_stays_outstanding_until_taken() {
        let dir = tempfile::tempdir().unwrap();
        let cached = dir.path().join("cached.png");
        std::fs::write(&cached, thumbs::png::blank()).unwrap();
        let ctx = egui::Context::default();
        let mut decoder = Decoder::new("test-decoder");
        assert!(decoder.ask(&"a", &cached, &ctx));
        assert!(decoder.ask(&"b", &cached, &ctx));
        // Both decoded; taking b collects a too, leaving it unclaimed.
        assert!(wait(|| decoder.take(&"b")).is_some());
        wait(|| decoder.done.iter().any(|(k, _)| *k == "a").then_some(()));
        let next = decoder.next;
        assert!(decoder.ask(&"a", &cached, &ctx));
        assert_eq!(decoder.next, next, "not decoded again");
        assert!(decoder.replace(&"a", &cached, &ctx));
        assert_eq!(decoder.next, next, "replace keeps it");
        assert!(decoder.take(&"a").expect("kept").is_some());
        assert!(!decoder.pending());
        // Replace drops another key's unclaimed decode, and asks anew.
        assert!(decoder.ask(&"c", &cached, &ctx));
        assert!(decoder.ask(&"d", &cached, &ctx));
        assert!(wait(|| decoder.take(&"d")).is_some());
        assert!(decoder.replace(&"e", &cached, &ctx));
        assert!(decoder.take(&"c").is_none(), "withdrawn");
        assert!(wait(|| decoder.take(&"e")).is_some());
        assert!(!decoder.pending());
    }

    /// Asking queues: every request is answered, in any order of taking.
    #[test]
    fn ask_answers_every_request() {
        let dir = tempfile::tempdir().unwrap();
        let cached = dir.path().join("cached.png");
        std::fs::write(&cached, thumbs::png::blank()).unwrap();
        let broken = dir.path().join("broken.png");
        std::fs::write(&broken, b"no").unwrap();
        let ctx = egui::Context::default();
        let mut decoder = Decoder::new("test-decoder");
        assert!(decoder.ask(&1, &cached, &ctx));
        assert!(decoder.ask(&2, &broken, &ctx));
        assert!(decoder.ask(&3, &cached, &ctx));
        let mut seen = Vec::new();
        wait(|| {
            seen.extend(decoder.drain());
            (seen.len() == 3).then_some(())
        });
        seen.sort_by_key(|(k, _)| *k);
        assert!(seen[0].1.is_some());
        assert!(seen[1].1.is_none(), "a broken entry answers None");
        assert!(seen[2].1.is_some());
        assert!(!decoder.pending());
    }
}
