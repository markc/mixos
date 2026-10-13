// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! What a spawned program may inherit.
//!
//! A process can itself inherit descriptors without close-on-exec: CI runners,
//! some launchers and careless parents hand them out (GitHub's runner leaves
//! fds 142 and 145 open). Every structured spawn (`run_argv`, `run_pipeline`,
//! `run_stream`, `spawn`, session tasks, desktop helpers) gives the child its
//! three standard streams plus whatever it deliberately maps, and nothing
//! else. The shell-syntax paths (`$(...)`, `sh` statements, `| external`) are
//! left alone: passing inherited descriptors through is a shell's contract.
//!
//! Descriptors are marked close-on-exec, never closed: std's exec-error pipe
//! lives above stderr and has to stay open until exec. A daemon that needs
//! inherited sockets (systemd socket activation) must be the unit's own
//! ExecStart, not a child of a Mix script.

/// The first descriptor above the standard streams.
pub const FIRST_INHERITED: i32 = 3;

/// How far the per-descriptor fallback looks on kernels without close_range.
const SWEEP_LIMIT: i32 = 4096;

/// Mark every descriptor from `first` up close-on-exec. Call it in a
/// `pre_exec` closure after any `dup2` onto a descriptor the child should
/// keep, with `first` above that descriptor.
///
/// # Safety
/// Only raw syscalls (close_range, fcntl): no allocation, no locks. Safe in
/// the post-fork, pre-exec window.
#[cfg(unix)]
pub unsafe fn cloexec_from(first: i32) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: close_range takes plain integers and touches only this
        // process's descriptor table.
        let done = unsafe {
            libc::syscall(
                libc::SYS_close_range,
                first as libc::c_uint,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC,
            )
        } == 0;
        if done {
            return;
        }
    }
    // Before Linux 5.11, and other unix: one descriptor at a time.
    for fd in first..SWEEP_LIMIT {
        // SAFETY: fcntl on a closed descriptor fails with EBADF and changes
        // nothing.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
}

/// Register [`cloexec_from`] for everything above the standard streams on
/// `command`. Closures run in registration order and std maps the standard
/// streams before any of them, so this composes with the caller's own
/// `pre_exec` setup.
#[cfg(unix)]
pub fn seal(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the closure only calls cloexec_from, which is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            cloexec_from(FIRST_INHERITED);
            Ok(())
        });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn a_sealed_child_sees_only_its_standard_streams() {
        // A descriptor this process holds without close-on-exec, as an
        // inherited one would be.
        let file = std::fs::File::open("/dev/null").unwrap();
        let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 200) };
        assert!(fd >= 200);
        let mut listing = std::process::Command::new("sh");
        listing.args(["-c", "ls /proc/self/fd 2>/dev/null || ls /dev/fd"]);
        let plain = listing.output().unwrap();
        let mut sealed = std::process::Command::new("sh");
        sealed.args(["-c", "ls /proc/self/fd 2>/dev/null || ls /dev/fd"]);
        seal(&mut sealed);
        let sealed = sealed.output().unwrap();
        unsafe { libc::close(fd) };
        let names = |out: &std::process::Output| -> Vec<i32> {
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect()
        };
        assert!(
            names(&plain).contains(&fd),
            "fixture: unsealed child must see fd {fd}"
        );
        assert!(
            !names(&sealed).contains(&fd),
            "sealed child saw fd {fd}: {:?}",
            names(&sealed)
        );
    }
}
