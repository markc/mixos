// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lifetime protection for processes owned by a test, never worker-wide cleanup.
use std::os::unix::process::CommandExt;
use std::process::Command;

/// Arm before any other pre_exec work. Check the fork/prctl race against the
/// parent's PID captured before fork, rather than accepting a new adopter.
pub fn guard(command: &mut Command) {
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

/// Includes job-control groups inside the session. Re-read stat before each
/// signal so an exited/reused PID cannot select a process from another job.
pub fn kill_session(session: u32) {
    #[cfg(target_os = "linux")]
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            let path = entry.path().join("stat");
            let Ok(stat) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some((_, tail)) = stat.rsplit_once(") ") else {
                continue;
            };
            let fields: Vec<_> = tail.split_whitespace().collect();
            if fields.get(3).and_then(|id| id.parse::<u32>().ok()) != Some(session) {
                continue;
            }
            let Ok(now) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some((_, tail)) = now.rsplit_once(") ") else {
                continue;
            };
            let current: Vec<_> = tail.split_whitespace().collect();
            if current.get(3) == fields.get(3) && current.get(19) == fields.get(19) {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    unsafe {
        libc::kill(-(session as i32), libc::SIGKILL)
    };
}
