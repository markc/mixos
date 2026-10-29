// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lifetime protection for processes owned by a test, never worker-wide cleanup.
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};

/// Original child identity survives a raw ptrace reap as a stale token rather
/// than accidentally authorising cleanup of a new child with a reused PID.
pub struct Session {
    pid: u32,
    start: Option<String>,
}

impl Session {
    pub fn new(child: &Child) -> Self {
        #[cfg(target_os = "linux")]
        let start = Some(identity(child.id()).expect("spawned session leader stat").1);
        #[cfg(not(target_os = "linux"))]
        let start = None;
        Self {
            pid: child.id(),
            start,
        }
    }

    pub fn kill(&self) {
        kill_session(self.pid, self.start.as_deref());
    }
}

#[cfg(target_os = "linux")]
fn identity(pid: u32) -> Option<(u32, String)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, tail) = stat.rsplit_once(") ")?;
    let fields: Vec<_> = tail.split_whitespace().collect();
    Some((fields.get(3)?.parse().ok()?, fields.get(19)?.to_string()))
}

/// Observe terminal status without surrendering the session leader's PID.
/// Drop must clean the fixture's other job groups before the final reap.
pub fn try_wait(child: &mut Child) -> std::io::Result<Option<ExitStatus>> {
    #[cfg(target_os = "linux")]
    return status(child.id(), true);
    #[cfg(not(target_os = "linux"))]
    child.try_wait()
}

pub fn wait(child: &mut Child) -> std::io::Result<ExitStatus> {
    #[cfg(target_os = "linux")]
    return status(child.id(), false)?.ok_or_else(|| std::io::Error::other("missing child status"));
    #[cfg(not(target_os = "linux"))]
    child.wait()
}

#[cfg(target_os = "linux")]
fn status(pid: u32, nonblocking: bool) -> std::io::Result<Option<ExitStatus>> {
    use std::os::unix::process::ExitStatusExt;
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let options = libc::WEXITED | libc::WNOWAIT | if nonblocking { libc::WNOHANG } else { 0 };
    if unsafe { libc::waitid(libc::P_PID, pid, &mut info, options) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { info.si_pid() } == 0 {
        return Ok(None);
    }
    let code = unsafe { info.si_status() };
    let raw = match info.si_code {
        libc::CLD_EXITED => code << 8,
        libc::CLD_DUMPED => code | 0x80,
        _ => code,
    };
    Ok(Some(ExitStatus::from_raw(raw)))
}

/// Arm before any other pre_exec work. Check the fork/prctl race against the
/// parent's PID captured before fork, rather than accepting a new adopter.
/// On Linux the signal follows the spawning thread's lifetime; keep these
/// fixture children scoped to that test thread and its Drop cleanup.
pub fn guard(command: &mut Command) {
    #[cfg(target_os = "linux")]
    let parent = std::process::id() as libc::pid_t;
    unsafe {
        command.pre_exec(move || {
            #[cfg(target_os = "linux")]
            {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    libc::_exit(128 + libc::SIGKILL);
                }
            }
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            Ok(())
        });
    }
}

/// Includes job-control groups inside an unreaped, directly owned session.
/// The leader stays reserved until cleanup; pidfds bind signals to the exact
/// selected members. Already-reaped or foreign leaders are never signalled.
fn kill_session(session: u32, expected_start: Option<&str>) {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        // waitid proves this is still our unreaped child, even after an exit
        // assertion. It returns ECHILD after a raw ptrace reaper consumed it.
        if status(session, true).is_err() {
            return;
        }
        let Some(leader) =
            identity(session).filter(|leader| Some(leader.1.as_str()) == expected_start)
        else {
            return;
        };
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                    continue;
                };
                let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
                if fd < 0 {
                    continue;
                }
                let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
                if identity(pid).is_some_and(|member| {
                    pid == session || (leader.0 == session && member.0 == session)
                })
                    && identity(session).as_ref() == Some(&leader)
                {
                    unsafe {
                        libc::syscall(
                            libc::SYS_pidfd_send_signal,
                            fd.as_raw_fd(),
                            libc::SIGKILL,
                            std::ptr::null::<libc::siginfo_t>(),
                            0,
                        );
                    }
                }
            }
        }
    }
    // MixOS and its strong PTY lifetime guarantees target Linux. Other Unix
    // builds cannot enumerate sessions safely and must not signal guessed PIDs.
    #[cfg(not(target_os = "linux"))]
    let _ = (session, expected_start);
}

#[cfg(target_os = "linux")]
#[test]
fn stale_session_identity_does_not_signal_a_new_leader() {
    let mut command = Command::new("sleep");
    guard(&mut command);
    command.arg("60");
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let session = Session::new(&child);
    let stale = Session {
        pid: child.id(),
        start: Some("stale leader start".into()),
    };
    stale.kill();
    let untouched = try_wait(&mut child).unwrap().is_none();
    session.kill();
    let _ = child.wait();
    assert!(untouched, "an old identity authorised signalling a new leader");
}

#[cfg(target_os = "linux")]
#[test]
fn cleanup_kills_only_owned_child_without_a_new_session() {
    let mut command = Command::new("sleep");
    guard(&mut command);
    let mut child = command.arg("60").spawn().unwrap();
    let session = Session::new(&child);
    assert_ne!(identity(child.id()).unwrap().0, child.id());
    session.kill();
    assert!(!child.wait().unwrap().success());
}
