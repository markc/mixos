// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real terminal loss, independent of delivery of SIGHUP.
#![cfg(target_os = "linux")]
#[path = "support/pty.rs"]
mod pty;

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

// openpty has no atomic CLOEXEC variant. Exclude every sibling fixture fork
// until its original descriptors have closed; like the other PTY suites,
// retain the lock for the complete fixture lifetime.
static FIXTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn fixture_guard() -> std::sync::MutexGuard<'static, ()> {
    FIXTURE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

struct Shell {
    child: Child,
    master: Option<File>,
    _home: tempfile::TempDir,
}
impl Drop for Shell {
    fn drop(&mut self) {
        pty::kill_session(self.child.id());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Shell {
    fn new(ignore_hup: bool, controlling: bool, editor: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join(".mixrc"),
            "fn prompt()\nreturn \"LOSS> \"\nend\n",
        )
        .unwrap();
        let (mut master, mut slave) = (-1, -1);
        let size = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    &size,
                )
            },
            0
        );
        for fd in [master, slave] {
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
        }
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mix"));
        pty::guard(&mut cmd);
        cmd.env("HOME", home.path())
            .env("MIX_STATS", "off")
            .env("MIX_EDITOR", editor)
            .env_remove("MIXOS_SESSION_FD")
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave);
        unsafe {
            cmd.pre_exec(move || {
                libc::signal(
                    libc::SIGHUP,
                    if ignore_hup {
                        libc::SIG_IGN
                    } else {
                        libc::SIG_DFL
                    },
                );
                if libc::setsid() < 0 || (controlling && libc::ioctl(0, libc::TIOCSCTTY, 0) < 0) {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Self {
            child: cmd.spawn().unwrap(),
            master: Some(master),
            _home: home,
        }
    }
    fn wait(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "shell {} survived terminal loss",
                self.child.id()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn prompt(&mut self) {
        self.until("LOSS> ");
    }
    fn until(&mut self, marker: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let master = self.master.as_mut().unwrap();
        let mut output = String::new();
        while !output.contains(marker) {
            assert!(Instant::now() < deadline, "prompt: {output:?}");
            let mut fd = libc::pollfd {
                fd: master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut fd, 1, 50) } <= 0 {
                continue;
            }
            let mut bytes = [0; 4096];
            let n = master.read(&mut bytes).unwrap();
            assert_ne!(n, 0);
            output.push_str(&String::from_utf8_lossy(&bytes[..n]));
            if output.contains("\x1b[6n") {
                master.write_all(b"\x1b[1;1R").unwrap();
            }
        }
    }
    fn job(&mut self, background: bool) -> i32 {
        let report = self._home.path().join("job-pid");
        self.master
            .as_mut()
            .unwrap()
            .write_all(
                format!(
                    "TERMINAL_JOB_REPORT={} {} --exact fixture_job --nocapture{}\n",
                    report.display(),
                    std::env::current_exe().unwrap().display(),
                    if background { " &" } else { "" }
                )
                .as_bytes(),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(text) = fs::read_to_string(&report)
                && let Ok(pid) = text.parse()
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "background job did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn closing_master_exits_with_and_without_inherited_hup_ignore() {
    let _fixture = fixture_guard();
    for editor in ["owned", "rustyline"] {
        for ignore_hup in [false, true] {
            let mut shell = Shell::new(ignore_hup, true, editor);
            shell.prompt();
            drop(shell.master.take());
            assert_eq!(
                shell.wait().code(),
                Some(129),
                "{editor}, ignored={ignore_hup}"
            );
        }
    }
}

#[test]
fn terminal_loss_with_incomplete_input_exits_129() {
    let _fixture = fixture_guard();
    for (controlling, editor) in [(true, "owned"), (true, "rustyline"), (false, "rustyline")] {
        let mut shell = Shell::new(true, controlling, editor);
        shell.prompt();
        shell
            .master
            .as_mut()
            .unwrap()
            .write_all(b"if true then\n")
            .unwrap();
        shell.until("  > ");
        drop(shell.master.take());
        assert_eq!(
            shell.wait().code(),
            Some(129),
            "{editor}, controlling={controlling}"
        );
    }
}

#[test]
fn terminal_loss_without_job_control_exits() {
    let _fixture = fixture_guard();
    // No controlling tty: no controller or HUP handler. The editor must
    // distinguish a disconnected terminal from temporary foreground loss.
    let mut shell = Shell::new(true, false, "owned");
    std::thread::sleep(Duration::from_millis(300));
    drop(shell.master.take());
    assert_eq!(shell.wait().code(), Some(129));
}

#[test]
fn plain_eof_exits_zero() {
    let _fixture = fixture_guard();
    let mut shell = Shell::new(false, true, "owned");
    shell.prompt();
    shell.master.as_mut().unwrap().write_all(b"\x04").unwrap();
    assert_eq!(shell.wait().code(), Some(0));
}

#[test]
fn fixture_job() {
    let _fixture = fixture_guard();
    let Some(report) = std::env::var_os("TERMINAL_JOB_REPORT") else {
        return;
    };
    fs::write(report, std::process::id().to_string()).unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn terminal_hangup_also_hangs_up_background_job_groups() {
    let _fixture = fixture_guard();
    let mut shell = Shell::new(true, true, "owned");
    shell.prompt();
    let child = shell.job(true);
    drop(shell.master.take());
    assert_eq!(shell.wait().code(), Some(129));
    assert!(
        !std::path::Path::new(&format!("/proc/{child}")).exists(),
        "background job {child} outlived the shell"
    );
}

#[test]
fn terminal_hangup_while_waiting_for_a_foreground_job_exits_129() {
    let _fixture = fixture_guard();
    let mut shell = Shell::new(true, true, "owned");
    shell.prompt();
    let child = shell.job(false);
    drop(shell.master.take());
    assert_eq!(shell.wait().code(), Some(129));
    assert!(!std::path::Path::new(&format!("/proc/{child}/exe")).exists());
}

#[test]
fn fixture_drop_kills_grandchildren_in_other_job_groups() {
    let _fixture = fixture_guard();
    let mut shell = Shell::new(false, true, "owned");
    shell.prompt();
    let child = shell.job(true);
    assert_ne!(unsafe { libc::getpgid(child) }, shell.child.id() as i32);
    assert_eq!(unsafe { libc::getsid(child) }, shell.child.id() as i32);
    drop(shell);
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::path::Path::new(&format!("/proc/{child}/exe")).exists() {
        assert!(
            Instant::now() < deadline,
            "fixture left background grandchild {child} alive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn fixture_parent() {
    let _fixture = fixture_guard();
    let Some(report) = std::env::var_os("TERMINAL_PARENT_REPORT") else {
        return;
    };
    let shell = Shell::new(true, false, "owned");
    // A stopped shell cannot notice terminal EOF or run a signal handler.
    // Its death therefore proves the kernel parent-death guard fired.
    assert_eq!(
        unsafe { libc::kill(shell.child.id() as i32, libc::SIGSTOP) },
        0
    );
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(shell.child.id() as i32, &mut status, libc::WUNTRACED) },
        shell.child.id() as i32
    );
    assert!(libc::WIFSTOPPED(status));
    fs::write(report, shell.child.id().to_string()).unwrap();
    // The outer test kills this process; normal return would run Shell::drop.
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[test]
fn killed_test_runner_cannot_orphan_a_stopped_shell() {
    let _fixture = fixture_guard();
    let home = tempfile::tempdir().unwrap();
    let report = home.path().join("child-pid");
    let mut command = Command::new(std::env::current_exe().unwrap());
    pty::guard(&mut command);
    command
        .args(["--exact", "fixture_parent", "--nocapture"])
        .env("TERMINAL_PARENT_REPORT", &report);
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut runner = Shell {
        child: command.spawn().unwrap(),
        master: None,
        _home: home,
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let child: i32 = loop {
        if let Ok(text) = fs::read_to_string(&report)
            && let Ok(pid) = text.parse()
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "runner did not report its child");
        std::thread::sleep(Duration::from_millis(10));
    };
    runner.child.kill().unwrap();
    runner.child.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = fs::read_to_string(format!("/proc/{child}/stat"));
        if state.is_err()
            || state
                .as_ref()
                .is_ok_and(|s| s.rsplit_once(") ").is_some_and(|(_, s)| s.starts_with('Z')))
        {
            break;
        }
        if Instant::now() >= deadline {
            pty::kill_session(child as u32);
            panic!("kernel parent-death guard left shell {child} alive");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
