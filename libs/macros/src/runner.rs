// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::{Entry, Roots};
use std::collections::VecDeque;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const OUTPUT_LIMIT: usize = 64 * 1024;

pub type FinishHook = Arc<dyn Fn(&Status) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Running,
    /// None means the process was terminated by a signal.
    Exited(Option<i32>),
    FailedToStart(String),
    Failed(String),
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
    finish_hook: Option<FinishHook>,
}

impl Default for Runner {
    fn default() -> Self {
        Self {
            status: Arc::new(Mutex::new(Status::Idle)),
            output: Arc::new(Mutex::new(Buffer::default())),
            stop: None,
            finish_hook: None,
        }
    }
}

impl Runner {
    pub fn set_finish_hook(&mut self, hook: Option<FinishHook>) {
        self.finish_hook = hook;
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
    pub fn output(&self) -> Output {
        let buffer = self.output.lock().unwrap();
        Output {
            text: String::from_utf8_lossy(&buffer.bytes.iter().copied().collect::<Vec<_>>())
                .into_owned(),
            dropped: buffer.dropped,
        }
    }

    pub fn start(&mut self, roots: &Roots, app: &str, entry: &Entry) -> io::Result<()> {
        if self.status() == Status::Running {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a macro is already running",
            ));
        }
        *self.status.lock().unwrap() = Status::Running;
        // A detached descendant may retain a pipe from an earlier run.
        self.output = Arc::new(Mutex::new(Buffer::default()));
        let (tx, rx) = mpsc::channel();
        let status = self.status.clone();
        let output = self.output.clone();
        let roots = roots.clone();
        let app = app.to_owned();
        let entry = entry.clone();
        let finish_hook = self.finish_hook.clone();
        let worker = thread::Builder::new()
            .name("macro-runner".into())
            .spawn(move || {
                let mut command = Command::new(&roots.mix);
                command
                    .arg(&entry.path)
                    .current_dir(&roots.home)
                    .env("MIXOS_MACRO_APP", app)
                    .env("MIXOS_MACRO_ID", &entry.id)
                    .env("MIXOS_MACRO_TIER", entry.tier.as_str())
                    .env("MIXOS_MACRO_PATH", &entry.path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
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
                    Err(err) => {
                        let final_status = Status::FailedToStart(err.to_string());
                        *status.lock().unwrap() = final_status.clone();
                        if let Some(hook) = &finish_hook {
                            hook(&final_status);
                        }
                        return;
                    }
                };
                let stdout = capture(child.stdout.take().unwrap(), output.clone());
                let stderr = capture(child.stderr.take().unwrap(), output);
                let result = loop {
                    match exited(&mut child) {
                        Ok(true) => {
                            terminate(&mut child);
                            break Ok(());
                        }
                        Err(err) => {
                            if err.kind() == io::ErrorKind::Interrupted {
                                continue;
                            }
                            // ECHILD means the PID is no longer reserved.
                            if err.raw_os_error() != Some(libc::ECHILD) {
                                terminate(&mut child);
                            }
                            break Err(err);
                        }
                        Ok(false) => {}
                    }
                    match rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            terminate(&mut child);
                            break Ok(());
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                };
                // Allow ordinary output to drain, but never wait indefinitely
                // for a descendant in another session holding the pipes open.
                let deadline = std::time::Instant::now() + Duration::from_millis(100);
                while !(stdout.is_finished() && stderr.is_finished())
                    && std::time::Instant::now() < deadline
                {
                    thread::sleep(Duration::from_millis(1));
                }
                // No signals are sent after this reap. Publish immediately;
                // dropping the reader handles detaches any lingering readers.
                let reaped = child.wait();
                let result = result.and(reaped);
                let final_status = match result {
                    Ok(exit) => Status::Exited(exit.code()),
                    Err(err) => Status::Failed(err.to_string()),
                };
                *status.lock().unwrap() = final_status.clone();
                if let Some(hook) = &finish_hook {
                    hook(&final_status);
                }
            });
        match worker {
            Ok(_worker) => {
                self.stop = Some(tx);
                Ok(())
            }
            Err(err) => {
                *self.status.lock().unwrap() = Status::FailedToStart(err.to_string());
                Err(err)
            }
        }
    }

    /// Request termination; the manager kills the process group and reaps it.
    pub fn stop(&self) {
        if let Some(stop) = &self.stop {
            let _ = stop.send(());
        }
    }
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
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = child.kill();
}

#[cfg(unix)]
fn exited(child: &mut Child) -> io::Result<bool> {
    // SAFETY: info is writable and waitid only observes our owned child.
    // WNOWAIT reserves its PID until group cleanup and Child::wait finish.
    let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id(),
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: waitid initialised the SIGCHLD fields (or left zero).
        Ok(unsafe { info.si_pid() } != 0)
    }
}

#[cfg(not(unix))]
fn exited(child: &mut Child) -> io::Result<bool> {
    child.try_wait().map(|status| status.is_some())
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
        assert!(
            buffer
                .bytes
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .ends_with(b"stderr\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn dropping_a_runner_requests_termination_and_the_manager_reaps() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let mix = temp.path().join("runner-stub");
        std::fs::write(
            &mix,
            "#!/bin/sh\n# SPDX-License-Identifier: MIT OR Apache-2.0\nexec /bin/sleep 120\n",
        )
        .unwrap();
        std::fs::set_permissions(&mix, std::fs::Permissions::from_mode(0o700)).unwrap();
        let roots = Roots {
            user: temp.path().join("user"),
            builtin: temp.path().join("builtin"),
            home: temp.path().into(),
            mix,
        };
        let path = temp.path().join("drop.mix");
        std::fs::write(&path, "-- title: Drop\n").unwrap();
        let entry = Entry {
            id: "drop".into(),
            title: "Drop".into(),
            tier: Tier::UserApp,
            path,
            shadows: vec![],
        };
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
