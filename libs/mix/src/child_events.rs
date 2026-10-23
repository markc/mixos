// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Managed children have one reaper. poll(2) waits on pidfd + cancellation FD,
//! with an infinite deadline for leader exit; bounded deadlines apply only to
//! cancellation grace and draining inherited pipes after exit.
use crate::{
    error::MixResult,
    native_events::{Queue, refusal},
};
use std::{process::Child, sync::Arc};

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::{net::UnixStream, process::ExitStatusExt},
        },
        thread,
    };

    /// Called only while the leader is unreaped, pinning its PID/group.
    fn open_pidfd(pid: i32) -> std::io::Result<OwnedFd> {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 };
        if fd < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    }

    fn signal_group(pid: i32, signal: i32) {
        unsafe {
            libc::kill(-pid, signal);
        }
    }

    fn signal_leader(pidfd: &OwnedFd, signal: i32) -> bool {
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                pidfd.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            ) == 0
        }
    }

    /// The same generation-owned, sole-reaper model as ChildWatch, with
    /// stdout/stderr and cancellation in one poll set. No evaluator values
    /// or runtime handles cross the worker boundary.
    pub(crate) struct ProcessWatch {
        cancel: UnixStream,
        completed: Arc<std::sync::atomic::AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    struct Pipe {
        fd: OwnedFd,
        stream: &'static str,
        line: Vec<u8>,
        truncated: bool,
        closed: bool,
    }

    impl Pipe {
        fn new(fd: OwnedFd, stream: &'static str) -> std::io::Result<Self> {
            let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
            if flags < 0
                || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                    < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self {
                fd,
                stream,
                line: Vec::new(),
                truncated: false,
                closed: false,
            })
        }

        fn emit(&mut self, queue: &Queue, handle: &str) {
            // Lossy UTF-8 is explicit in the manual; retained storage stays
            // bounded even for an indefinitely long unterminated line.
            queue.process_line(
                handle,
                self.stream,
                String::from_utf8_lossy(&self.line).into_owned(),
                self.truncated,
            );
            self.line.clear();
            self.truncated = false;
        }

        fn read(&mut self, queue: &Queue, handle: &str, max: usize) {
            let mut bytes = [0; 4096];
            let n =
                unsafe { libc::read(self.fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
            if n > 0 {
                for &b in &bytes[..n as usize] {
                    if b == b'\n' {
                        self.emit(queue, handle);
                    } else if self.line.len() < max {
                        self.line.push(b);
                    } else {
                        self.truncated = true;
                    }
                }
            } else if n == 0
                || !matches!(
                    std::io::Error::last_os_error().kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                )
            {
                if !self.line.is_empty() || self.truncated {
                    self.emit(queue, handle);
                }
                self.closed = true;
            }
        }
    }

    impl ProcessWatch {
        pub fn new(
            mut command: std::process::Command,
            queue: Arc<Queue>,
            handle: String,
            max_line: usize,
        ) -> MixResult<Self> {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
            crate::desktop_events::arm_parent_death(&mut command);
            let mut child = command
                .spawn()
                .map_err(|e| refusal("PROCESS_SPAWN", e.to_string()))?;
            let pid = child.id() as i32;
            let setup = (|| -> std::io::Result<_> {
                let pidfd = open_pidfd(pid)?;
                let (cancel, wake) = UnixStream::pair()?;
                let mut pipes = vec![Pipe::new(child.stdout.take().unwrap().into(), "stdout")?];
                if let Some(stderr) = child.stderr.take() {
                    pipes.push(Pipe::new(stderr.into(), "stderr")?);
                }
                Ok((pidfd, cancel, wake, pipes))
            })();
            let (pidfd, cancel, wake, mut pipes) = match setup {
                Ok(v) => v,
                Err(e) => {
                    signal_group(pid, libc::SIGKILL);
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(refusal("PROC_MONITOR", e.to_string()));
                }
            };
            crate::builtins::register_managed_pid(pid);
            let slot = Arc::new(std::sync::Mutex::new(Some(child)));
            let worker_slot = slot.clone();
            let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let worker_completed = completed.clone();
            let worker = thread::Builder::new()
                .name("mix-process-lines".into())
                .spawn(move || {
                    let mut child = worker_slot.lock().unwrap().take().unwrap();
                    let mut exited = false;
                    let mut cancelled = false;
                    let mut drain_deadline = None;
                    let mut detached_output = false;
                    loop {
                        let timeout = drain_deadline.map_or(-1, |deadline: std::time::Instant| {
                            deadline
                                .saturating_duration_since(std::time::Instant::now())
                                .as_millis()
                                .min(500) as i32
                        });
                        if timeout == 0 {
                            detached_output = pipes.iter().any(|p| !p.closed);
                            for pipe in &mut pipes {
                                if !pipe.closed && (!pipe.line.is_empty() || pipe.truncated) {
                                    pipe.emit(&queue, &handle);
                                }
                            }
                            break;
                        }
                        let mut fds = vec![
                            libc::pollfd {
                                fd: if exited { -1 } else { pidfd.as_raw_fd() },
                                events: libc::POLLIN,
                                revents: 0,
                            },
                            libc::pollfd {
                                fd: wake.as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0,
                            },
                        ];
                        for pipe in &pipes {
                            fds.push(libc::pollfd {
                                fd: if pipe.closed { -1 } else { pipe.fd.as_raw_fd() },
                                events: libc::POLLIN,
                                revents: 0,
                            });
                        }
                        let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
                        if rc < 0
                            && std::io::Error::last_os_error().kind()
                                == std::io::ErrorKind::Interrupted
                        {
                            continue;
                        }
                        if rc < 0 || fds[1].revents != 0 {
                            cancelled = true;
                            // Keep the leader unreaped during grace: its PID pins
                            // the group, even when it exits immediately on TERM.
                            signal_group(pid, libc::SIGTERM);
                            signal_leader(&pidfd, libc::SIGTERM);
                            let deadline =
                                std::time::Instant::now() + std::time::Duration::from_millis(150);
                            let mut grace = libc::pollfd {
                                fd: pidfd.as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0,
                            };
                            loop {
                                let remaining = deadline
                                    .saturating_duration_since(std::time::Instant::now())
                                    .as_millis()
                                    as i32;
                                if remaining == 0
                                    || unsafe { libc::poll(&mut grace, 1, remaining) } >= 0
                                {
                                    break;
                                }
                                if std::io::Error::last_os_error().kind()
                                    != std::io::ErrorKind::Interrupted
                                {
                                    break;
                                }
                            }
                            signal_group(pid, libc::SIGKILL);
                            signal_leader(&pidfd, libc::SIGKILL);
                            break;
                        }
                        if fds[0].revents != 0 {
                            exited = true;
                            drain_deadline = Some(
                                std::time::Instant::now() + std::time::Duration::from_millis(500),
                            );
                            // End descendants in the original group. Detached
                            // pipe holders are bounded by the drain deadline.
                            signal_group(pid, libc::SIGKILL);
                        }
                        for (pipe, fd) in pipes.iter_mut().zip(&fds[2..]) {
                            if fd.revents != 0 {
                                pipe.read(&queue, &handle, max_line);
                            }
                        }
                        if exited && pipes.iter().all(|p| p.closed) {
                            break;
                        }
                    }
                    drop(pipes);
                    let status = child.wait();
                    crate::builtins::unregister_managed_pid(pid);
                    // Publish completion before notifying the consumer. Pruning
                    // joins this worker, so it cannot observe a live owner with
                    // an already drained terminal and park without another wake.
                    worker_completed.store(true, std::sync::atomic::Ordering::Release);
                    if !cancelled {
                        match status {
                            Ok(s) => {
                                queue.process_exited(&handle, s.code(), s.signal(), detached_output)
                            }
                            Err(_) => queue.process_exited(&handle, None, None, detached_output),
                        }
                    }
                });
            match worker {
                Ok(worker) => Ok(Self {
                    cancel,
                    completed,
                    worker: Some(worker),
                }),
                Err(e) => {
                    if let Some(mut child) = slot.lock().unwrap().take() {
                        signal_group(pid, libc::SIGKILL);
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    crate::builtins::unregister_managed_pid(pid);
                    Err(refusal("PROC_MONITOR", e.to_string()))
                }
            }
        }
        pub fn finished(&self) -> bool {
            self.completed.load(std::sync::atomic::Ordering::Acquire)
        }
    }

    impl Drop for ProcessWatch {
        fn drop(&mut self) {
            let _ = self.cancel.shutdown(std::net::Shutdown::Write);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    pub(crate) struct ChildWatch {
        pub pid: u32,
        pidfd: Arc<OwnedFd>,
        cancel: UnixStream,
        completed: Arc<std::sync::atomic::AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl ChildWatch {
        pub fn new(mut child: Child, tag: String, queue: Arc<Queue>) -> MixResult<Self> {
            let pid = child.id() as i32;
            // The child is unreaped, so its PID cannot be reused before pidfd_open.
            let pidfd = match open_pidfd(pid) {
                Ok(fd) => Arc::new(fd),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(refusal(
                        "PROC_UNSUPPORTED",
                        format!("managed spawn requires pidfd: {error}"),
                    ));
                }
            };
            let (cancel, wake) = match UnixStream::pair() {
                Ok(pair) => pair,
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(refusal("PROC_MONITOR", e.to_string()));
                }
            };
            crate::builtins::register_managed_pid(pid);
            // Keep ownership recoverable if thread creation itself fails.
            let slot = Arc::new(std::sync::Mutex::new(Some(child)));
            let worker_slot = slot.clone();
            let worker_fd = pidfd.clone();
            let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let worker_completed = completed.clone();
            let worker = thread::Builder::new().name("mix-child-exit".into()).spawn(move || {
                let mut child = worker_slot.lock().unwrap().take().unwrap();
                let mut fds = [
                    libc::pollfd { fd: worker_fd.as_raw_fd(), events: libc::POLLIN, revents: 0 },
                    libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
                ];
                let cancelled = loop {
                    let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
                    if rc < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted { continue }
                    // Cancellation wins simultaneous readiness. Still unreaped here:
                    // the group leader's PID is pinned, so -pid cannot name a reused group.
                    break rc < 0 || fds[1].revents != 0;
                };
                // End remaining descendants before reaping the leader, even on
                // a natural exit. The unreaped PID pins the process-group identity.
                signal_group(pid, libc::SIGKILL);
                // The leader may have left its original process group. Signal
                // its retained identity too before the blocking reap; a group
                // kill alone cannot bound cancellation by the child's death.
                if cancelled {
                    signal_leader(&worker_fd, libc::SIGKILL);
                }
                let result = child.wait();
                crate::builtins::unregister_managed_pid(pid);
                // Publish completion before waking the consumer. Pruning joins
                // this worker, so its terminal record is queued before the
                // registry decides whether any sources remain.
                worker_completed.store(true, std::sync::atomic::Ordering::Release);
                if !cancelled {
                    match result {
                        Ok(status) => queue.child(serde_json::json!({
                            "pid": pid, "tag": tag, "exit_code": status.code(), "signal": status.signal()
                        })),
                        Err(e) => queue.child(serde_json::json!({
                            "pid": pid, "tag": tag, "exit_code": null, "signal": null,
                            "error_code": "PROC_REAP", "message": e.to_string()
                        })),
                    }
                }
            });
            match worker {
                Ok(worker) => Ok(Self {
                    pid: pid as u32,
                    pidfd,
                    cancel,
                    completed,
                    worker: Some(worker),
                }),
                Err(e) => {
                    if let Some(mut child) = slot.lock().unwrap().take() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    crate::builtins::unregister_managed_pid(pid);
                    Err(refusal("PROC_MONITOR", e.to_string()))
                }
            }
        }

        pub fn finished(&self) -> bool {
            self.completed.load(std::sync::atomic::Ordering::Acquire)
        }

        pub fn signal(&self, signal: i32) -> bool {
            signal_leader(&self.pidfd, signal)
        }
    }

    impl Drop for ChildWatch {
        fn drop(&mut self) {
            let _ = self.cancel.shutdown(std::net::Shutdown::Write);
            if let Some(w) = self.worker.take() {
                let _ = w.join();
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) use linux::{ChildWatch, ProcessWatch};

#[cfg(not(target_os = "linux"))]
pub(crate) struct ProcessWatch;
#[cfg(not(target_os = "linux"))]
impl ProcessWatch {
    pub fn new(_: std::process::Command, _: Arc<Queue>, _: String, _: usize) -> MixResult<Self> {
        Err(refusal(
            "PROC_UNSUPPORTED",
            "process_watch requires Linux pidfd",
        ))
    }
    pub fn finished(&self) -> bool {
        true
    }
}
#[cfg(not(target_os = "linux"))]
pub(crate) struct ChildWatch {
    pub pid: u32,
}
#[cfg(not(target_os = "linux"))]
impl ChildWatch {
    pub fn new(mut child: Child, _: String, _: Arc<Queue>) -> MixResult<Self> {
        let _ = child.kill();
        let _ = child.wait();
        Err(refusal(
            "PROC_UNSUPPORTED",
            "managed spawn requires Linux pidfd",
        ))
    }
    pub fn finished(&self) -> bool {
        true
    }
    pub fn signal(&self, _: i32) -> bool {
        false
    }
}
