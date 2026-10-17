// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `read_file` / `write_file` / `append_file` on a FIFO, without stalling
//! the runtime thread.
//!
//! The ordinary builtins are synchronous: on a FIFO nobody has opened from
//! the other end, `open(2)` blocks, and so does the single current-thread
//! runtime with it. While it was blocked a script ignored SIGTERM until the
//! 15 s backstop forced it out (no graceful shutdown), and a `--serve`
//! citizen answered nothing — not `QUIT`, not any other handler.
//!
//! A path that is a FIFO takes this path instead: the descriptor is opened
//! `O_NONBLOCK` and every wait is a zero-timeout `poll(2)` followed by a
//! short tokio sleep, so the evaluator yields while it waits. Shutdown
//! arms, Class C peers and the serve pump all keep running, dropping the
//! future abandons the wait cleanly, and a SIGINT ends the wait with an
//! `interrupted` error. Regular files never come here: the only cost on
//! the ordinary path is the one `stat(2)` that recognises a FIFO.
//!
//! A path that blocks for other reasons (a stuck NFS/FUSE mount, a
//! character device) still blocks the thread; the SIGTERM backstop remains
//! the bound there.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::time::Duration;

/// How long an idle wait sleeps between readiness checks.
const IDLE: Duration = Duration::from_millis(10);

/// Whether `path` names a FIFO (following symlinks, as `open` would).
pub(crate) fn is_fifo(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.file_type().is_fifo())
}

fn interrupted() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "interrupted")
}

/// Wait until `fd` reports any of `events` (or hangup/error), yielding to
/// the runtime between checks.
async fn ready(fd: i32, events: libc::c_short) -> std::io::Result<()> {
    loop {
        // Interruption first: a peer that is always ready must not keep a
        // Ctrl-C from being seen.
        if crate::interrupt::is_interrupted() {
            return Err(interrupted());
        }
        let mut poll = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: one valid pollfd, zero timeout.
        let n = unsafe { libc::poll(&mut poll, 1, 0) };
        if n > 0 {
            return Ok(());
        }
        if n < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        tokio::time::sleep(IDLE).await;
    }
}

/// Bytes moved in one burst before the transfer yields to the runtime, so a
/// peer that never stops producing (or consuming) cannot hold the thread.
const BURST: usize = 256 * 1024;

/// Read a FIFO to end of file: wait for a writer, then read until every
/// writer has closed — what a blocking `read_to_end` does, without
/// blocking. With `limit`, reading stops once more than `limit` bytes are
/// in hand; the caller refuses the oversized result.
pub(crate) async fn read(path: &str, limit: Option<usize>) -> std::io::Result<Vec<u8>> {
    // A read-only non-blocking open of a FIFO succeeds at once. Until a
    // writer has come and gone, poll reports neither data nor hangup, so
    // the zero-length read that means "no writer yet" is never mistaken
    // for end of file.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let fd = file.as_raw_fd();
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        ready(fd, libc::POLLIN).await?;
        let mut burst = 0;
        while burst < BURST {
            match file.read(&mut buf) {
                Ok(0) => return Ok(out),
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    burst += n;
                    if limit.is_some_and(|max| out.len() > max) {
                        return Ok(out);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        if burst >= BURST {
            tokio::task::yield_now().await;
        }
    }
}

/// Write `data` to a FIFO: wait for a reader, then write it all.
pub(crate) async fn write(path: &str, data: &[u8]) -> std::io::Result<()> {
    // A write-only non-blocking open of a FIFO with no reader fails ENXIO
    // instead of blocking; retry until a reader appears.
    let mut file = loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => break file,
            Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
                if crate::interrupt::is_interrupted() {
                    return Err(interrupted());
                }
                tokio::time::sleep(IDLE).await;
            }
            Err(e) => return Err(e),
        }
    };
    let fd = file.as_raw_fd();
    let mut rest = data;
    let mut burst = 0;
    while !rest.is_empty() {
        if burst >= BURST {
            burst = 0;
            if crate::interrupt::is_interrupted() {
                return Err(interrupted());
            }
            tokio::task::yield_now().await;
        }
        match file.write(rest) {
            Ok(n) => {
                rest = &rest[n..];
                burst += n;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                ready(fd, libc::POLLOUT).await?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private scratch directory, removed on drop.
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "mix-fifo-io-{}-{:032x}",
                std::process::id(),
                rand::random::<u128>()
            ));
            std::fs::create_dir(&dir).unwrap();
            Scratch(dir)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fifo(dir: &std::path::Path) -> String {
        let path = dir.join("pipe");
        let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        path.to_str().unwrap().to_string()
    }

    /// The FIFO waits poll the process-wide interrupt flag, which other
    /// tests set and hold. Hold the shared guard, and start from a clear
    /// flag, for the whole test.
    fn interrupt_guard() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::interrupt::TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::interrupt::_test_clear();
        guard
    }

    #[test]
    fn only_a_fifo_takes_this_path() {
        let dir = Scratch::new();
        let plain = dir.path().join("plain");
        std::fs::write(&plain, "x").unwrap();
        assert!(!is_fifo(plain.to_str().unwrap()));
        assert!(!is_fifo(dir.path().join("missing").to_str().unwrap()));
        assert!(is_fifo(&fifo(dir.path())));
    }

    /// Run one async test body on a fresh current-thread runtime. The guard
    /// is taken outside the runtime: it is never held across an `.await`.
    fn block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    #[test]
    fn a_waiting_read_yields_to_the_runtime() {
        let _guard = interrupt_guard();
        block_on(async {
        let dir = Scratch::new();
        let path = fifo(dir.path());
        let writer_path = path.clone();
        // The writer runs on THIS thread, so the read must yield for it to
        // run at all.
        let (got, ()) = tokio::join!(read(&path, None), async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            write(&writer_path, b"hello").await.unwrap();
        });
        assert_eq!(got.unwrap(), b"hello");
        });
    }

    #[test]
    fn a_waiting_write_is_abandoned_cleanly() {
        let _guard = interrupt_guard();
        block_on(async {
        let dir = Scratch::new();
        let path = fifo(dir.path());
        let outcome = tokio::time::timeout(Duration::from_millis(100), write(&path, b"x")).await;
        assert!(
            outcome.is_err(),
            "no reader: the write must still be waiting"
        );
        });
    }

    #[test]
    fn a_producer_that_never_stops_cannot_hold_the_thread() {
        let _guard = interrupt_guard();
        block_on(async {
        let dir = Scratch::new();
        let path = fifo(dir.path());
        let writer_path = path.clone();
        // A separate OS thread that writes as fast as it can, for ever.
        let writer = std::thread::spawn(move || {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .open(writer_path)
                .unwrap();
            let chunk = vec![0u8; 64 * 1024];
            while f.write_all(&chunk).is_ok() {}
        });
        let ticks = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let counter = ticks.clone();
        let local = tokio::task::LocalSet::new();
        let got = local
            .run_until(async move {
                tokio::task::spawn_local(async move {
                    loop {
                        counter.set(counter.get() + 1);
                        tokio::task::yield_now().await;
                    }
                });
                read(&path, Some(8 * 1024 * 1024)).await.unwrap()
            })
            .await;
        drop(local);
        writer.join().unwrap();
        assert!(got.len() > 8 * 1024 * 1024);
        assert!(ticks.get() > 0, "the read never yielded to a peer task");
        });
    }
}
