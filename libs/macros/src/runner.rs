// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::{Entry, Roots};
use std::collections::VecDeque;
use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Running,
    /// None means the process was terminated by a signal.
    Exited(Option<i32>),
    FailedToStart(String),
}

#[derive(Clone, Debug, Default)]
pub struct Output {
    pub text: String,
    pub dropped: bool,
}

#[derive(Default)]
struct Buffer {
    bytes: VecDeque<u8>,
    dropped: bool,
}

impl Buffer {
    fn append(&mut self, bytes: &[u8]) {
        self.bytes.extend(bytes);
        if self.bytes.len() > OUTPUT_LIMIT {
            self.bytes.drain(..self.bytes.len() - OUTPUT_LIMIT);
            self.dropped = true;
        }
    }
}

/// The manager thread owns and reaps the child. UI methods never wait on it.
pub struct Runner {
    status: Arc<Mutex<Status>>,
    output: Arc<Mutex<Buffer>>,
    stop: Option<mpsc::Sender<()>>,
}

impl Default for Runner {
    fn default() -> Self {
        Self { status: Arc::new(Mutex::new(Status::Idle)), output: Arc::new(Mutex::new(Buffer::default())), stop: None }
    }
}

impl Runner {
    pub fn status(&self) -> Status { self.status.lock().unwrap().clone() }
    pub fn output(&self) -> Output {
        let buffer = self.output.lock().unwrap();
        Output { text: String::from_utf8_lossy(&buffer.bytes.iter().copied().collect::<Vec<_>>()).into_owned(), dropped: buffer.dropped }
    }

    pub fn start(&mut self, roots: &Roots, app: &str, entry: &Entry) -> io::Result<()> {
        if self.status() == Status::Running {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a macro is already running"));
        }
        // A terminal status is published only after child and readers are
        // reaped. The previous manager may still be returning from its closure.
        *self.status.lock().unwrap() = Status::Running;
        *self.output.lock().unwrap() = Buffer::default();
        let (tx, rx) = mpsc::channel();
        let status = self.status.clone();
        let output = self.output.clone();
        let roots = roots.clone();
        let app = app.to_owned();
        let entry = entry.clone();
        let worker = thread::Builder::new().name("macro-runner".into()).spawn(move || {
            let mut command = Command::new(&roots.mix);
            command.arg(&entry.path).current_dir(&roots.home)
                .env("MIXOS_MACRO_APP", app).env("MIXOS_MACRO_ID", &entry.id)
                .env("MIXOS_MACRO_TIER", entry.tier.as_str()).env("MIXOS_MACRO_PATH", &entry.path)
                .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
            #[cfg(unix)]
            command.process_group(0);
            #[cfg(target_os = "linux")]
            {
                let parent = std::process::id() as libc::pid_t;
                // SAFETY: only async-signal-safe syscalls run between fork
                // and exec. The parent-death signal also covers application
                // exit before the background reaper can handle Drop's stop.
                unsafe {
                    command.pre_exec(move || {
                        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                            return Err(io::Error::last_os_error());
                        }
                        if libc::getppid() != parent {
                            return Err(io::Error::from_raw_os_error(libc::EINTR));
                        }
                        Ok(())
                    });
                }
            }
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(err) => { *status.lock().unwrap() = Status::FailedToStart(err.to_string()); return; }
            };
            let stdout = capture(child.stdout.take().unwrap(), output.clone());
            let stderr = capture(child.stderr.take().unwrap(), output);
            let result = loop {
                match child.try_wait() {
                    Ok(Some(exit)) => break Ok(exit),
                    Err(err) => { terminate(&mut child); let _ = child.wait(); break Err(err); }
                    Ok(None) => {}
                }
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        terminate(&mut child);
                        break child.wait();
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            };
            // Descendants must not keep the output pipes or outlive the macro.
            terminate(&mut child);
            let _ = stdout.join();
            let _ = stderr.join();
            *status.lock().unwrap() = match result {
                Ok(exit) => Status::Exited(exit.code()),
                Err(err) => Status::FailedToStart(err.to_string()),
            };
        });
        match worker {
            Ok(_worker) => { self.stop = Some(tx); Ok(()) }
            Err(err) => { *self.status.lock().unwrap() = Status::FailedToStart(err.to_string()); Err(err) }
        }
    }

    /// Request termination; the manager kills the process group and reaps it.
    pub fn stop(&self) { if let Some(stop) = &self.stop { let _ = stop.send(()); } }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop();
        // Detach the manager, which still owns the child and always reaps it.
        // Joining here would block an application's UI during shutdown.
    }
}

fn capture(mut pipe: impl Read + Send + 'static, output: Arc<Mutex<Buffer>>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut bytes = [0; 4096];
        loop {
            match pipe.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => output.lock().unwrap().append(&bytes[..n]),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    })
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    // SAFETY: the child starts its own process group; a negative pid targets it.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL); }
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tier;
    use std::time::Instant;

    #[test]
    fn buffer_keeps_exactly_the_last_64_kib_across_both_streams() {
        let mut buffer = Buffer::default();
        buffer.append(&vec![b'a'; OUTPUT_LIMIT]);
        assert!(!buffer.dropped);
        buffer.append(b"stderr\n");
        assert!(buffer.dropped);
        assert_eq!(buffer.bytes.len(), OUTPUT_LIMIT);
        assert!(buffer.bytes.iter().copied().collect::<Vec<_>>().ends_with(b"stderr\n"));
    }

    #[test]
    fn dropping_a_runner_requests_termination_and_the_manager_reaps() {
        let Some(mut roots) = Roots::resolve() else { return; };
        if !roots.mix.is_file() {
            eprintln!("skip: real Mix binary absent at {}", roots.mix.display());
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        roots.home = temp.path().into();
        let path = temp.path().join("drop.mix");
        std::fs::write(&path, "while true\nsleep(1)\nend\n").unwrap();
        let entry = Entry { id: "drop".into(), title: "Drop".into(), tier: Tier::UserApp, path, shadows: vec![] };
        let mut runner = Runner::default();
        let status = runner.status.clone();
        runner.start(&roots, "ced", &entry).unwrap();
        drop(runner);
        let deadline = Instant::now() + Duration::from_secs(10);
        while *status.lock().unwrap() == Status::Running {
            assert!(Instant::now() < deadline, "dropped runner failed to reap");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(*status.lock().unwrap(), Status::Exited(_)));
    }
}
