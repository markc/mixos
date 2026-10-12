// SPDX-License-Identifier: MIT OR Apache-2.0
//! Who is calling. Identity comes from the kernel (`SO_PEERCRED`) and the
//! caller's cgroup, never from anything the caller sends.
use serde::{Deserialize, Serialize};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use tokio::net::UnixStream;

/// The caller as seen for one request.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Caller {
    pub uid: u32,
    pub gid: u32,
    pub pid: Option<i32>,
    /// The unified-hierarchy cgroup path, such as
    /// `/machine.slice/systemd-nspawn@example.service/payload`.
    pub cgroup: Option<String>,
    /// True when the cgroup provably belongs to the connecting process: the
    /// kernel gave hostd a pidfd for it (`SO_PEERPIDFD`), and that process
    /// was still alive after the cgroup was read, so its pid was not reused
    /// in between. A grant is never matched on an unverified cgroup.
    pub verified: bool,
    /// True when the caller shares hostd's pid and user namespaces: it runs
    /// on the host itself, not inside a container. Proven the same way.
    #[serde(default)]
    pub host_ns: bool,
}

impl Caller {
    /// Root on the host itself: uid 0, verified, and outside any container.
    /// Container root is not this, with or without uid remapping.
    pub fn host_root(&self) -> bool {
        self.uid == 0 && self.verified && self.host_ns
    }
}

/// Where a request came in.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "via", rename_all = "snake_case")]
pub enum Origin {
    Local(Caller),
    /// The Bus carries reads only; mesh writes come later, signed.
    Bus,
}

/// The process at the other end of a connection, held for the connection's
/// life. Its cgroup and namespaces are read afresh for every request, so a
/// process that moves out of a granted cgroup loses the grant at once, and a
/// connection whose connecting process has exited proves nothing.
pub struct Peer {
    uid: u32,
    gid: u32,
    pid: Option<i32>,
    pidfd: Option<OwnedFd>,
}

impl Peer {
    pub fn of(stream: &UnixStream) -> std::io::Result<Self> {
        let cred = stream.peer_cred()?;
        Ok(Self {
            uid: cred.uid(),
            gid: cred.gid(),
            pid: cred.pid(),
            pidfd: peer_pidfd(stream),
        })
    }

    /// The caller as it stands now. Everything about the process is read
    /// first, then the pidfd proves the process still exists, so none of it
    /// came from a process that reused the pid.
    pub fn caller(&self) -> Caller {
        let cgroup = self
            .pid
            .and_then(|pid| std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok())
            .and_then(|text| unified_cgroup(&text));
        let same_ns = self.pid.is_some_and(same_namespaces);
        let verified = cgroup.is_some() && self.pidfd.as_ref().is_some_and(alive);
        Caller {
            uid: self.uid,
            gid: self.gid,
            pid: self.pid,
            cgroup,
            verified,
            host_ns: verified && same_ns,
        }
    }
}

/// Whether `pid` shares hostd's pid and user namespaces, from files any
/// process may read. (The `/proc/<pid>/ns/*` links need ptrace access, which
/// hostd, running with no capabilities, lacks for a caller holding more.)
///
/// - **pid:** `NSpid:` in `/proc/<pid>/status` lists the process's pid in
///   each pid namespace from the reader's down to its own. One entry means
///   the same pid namespace as hostd; a container's process has more.
/// - **user:** a process in hostd's user namespace has hostd's uid map. A
///   container's user namespace maps a narrower range, and only a process
///   already privileged on the host can make one with the full map.
fn same_namespaces(pid: i32) -> bool {
    let read = |path: String| std::fs::read_to_string(path).ok();
    let one_pid_ns =
        read(format!("/proc/{pid}/status")).and_then(|status| nspid_depth(&status)) == Some(1);
    let same_users = match (
        read(format!("/proc/{pid}/uid_map")),
        read("/proc/self/uid_map".into()),
    ) {
        (Some(theirs), Some(ours)) => normalise_map(&theirs) == normalise_map(&ours),
        _ => false,
    };
    one_pid_ns && same_users
}

/// How many pid namespaces `NSpid:` lists.
pub fn nspid_depth(status: &str) -> Option<usize> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("NSpid:"))
        .map(|rest| rest.split_whitespace().count())
}

/// A uid map with its column padding removed.
fn normalise_map(map: &str) -> Vec<Vec<&str>> {
    map.lines()
        .map(|line| line.split_whitespace().collect())
        .filter(|fields: &Vec<&str>| !fields.is_empty())
        .collect()
}

/// `SO_PEERPIDFD` (Linux 6.5); not every libc release names it.
const SO_PEERPIDFD: libc::c_int = 77;

/// A pidfd for the process that connected, when the kernel supports it.
fn peer_pidfd(stream: &UnixStream) -> Option<OwnedFd> {
    let mut fd: libc::c_int = -1;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `len` bytes into `fd`, which is a
    // c_int of exactly that size; the socket fd is valid for this call.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            SO_PEERPIDFD,
            (&mut fd as *mut libc::c_int).cast(),
            &mut len,
        )
    };
    // SAFETY: on success the kernel returned a new fd that we now own.
    (rc == 0 && fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Signal 0 through the pidfd: succeeds while the process exists (a zombie
/// included, whose pid is not yet free for reuse). hostd runs without
/// `CAP_KILL`, so another user's process answers `EPERM`, which also proves
/// it exists; only `ESRCH` means it is gone.
fn alive(pidfd: &OwnedFd) -> bool {
    // SAFETY: pidfd_send_signal with signal 0 and no siginfo only checks
    // that the process exists; the fd is a valid pidfd we own.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            0,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The cgroup v2 entry (`0::<path>`) of `/proc/<pid>/cgroup`.
pub fn unified_cgroup(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nspid_counts_pid_namespaces() {
        assert_eq!(nspid_depth("Name:\tx\nNSpid:\t42\n"), Some(1));
        assert_eq!(nspid_depth("NSpid:\t4242\t17\t1\n"), Some(3));
        assert_eq!(nspid_depth("Name:\tx\n"), None);
        assert_eq!(
            normalise_map("         0          0 4294967295\n"),
            normalise_map("0 0 4294967295")
        );
        assert_ne!(
            normalise_map("0 100000 65536\n"),
            normalise_map("0 0 4294967295\n")
        );
    }

    #[test]
    fn unified_cgroup_takes_the_v2_line() {
        let text =
            "1:name=systemd:/legacy\n0::/machine.slice/systemd-nspawn@example.service/payload\n";
        assert_eq!(
            unified_cgroup(text).as_deref(),
            Some("/machine.slice/systemd-nspawn@example.service/payload")
        );
        assert_eq!(unified_cgroup("1:cpu:/x\n"), None);
    }

    #[tokio::test]
    async fn peer_is_this_process() {
        let (a, _b) = UnixStream::pair().unwrap();
        let caller = Peer::of(&a).unwrap().caller();
        assert_eq!(caller.uid, unsafe { libc::getuid() });
        assert_eq!(caller.pid, Some(std::process::id() as i32));
        // Every supported kernel (6.5+) gives a pidfd for a live peer.
        assert!(caller.cgroup.is_some());
        assert!(caller.verified);
        // The test runs on the host side of its own namespaces.
        assert!(caller.host_ns);
        assert_eq!(caller.host_root(), caller.uid == 0);
    }

    /// A connection whose connecting process has gone proves nothing, even
    /// if its pid is free (or reused) by the time a request arrives.
    #[test]
    fn a_dead_connector_is_unverified() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        // SAFETY: pidfd_open on a child we have not yet reaped.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as libc::c_int;
        assert!(fd >= 0);
        child.wait().unwrap();
        let peer = Peer {
            uid: 0,
            gid: 0,
            pid: Some(pid),
            // SAFETY: a fresh pidfd we own.
            pidfd: Some(unsafe { OwnedFd::from_raw_fd(fd) }),
        };
        let caller = peer.caller();
        assert!(!caller.verified);
        assert!(!caller.host_root());
    }
}
