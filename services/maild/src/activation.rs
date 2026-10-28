// SPDX-License-Identifier: MIT OR Apache-2.0
//! Named systemd TCP listeners. Only the serve command consumes the inherited
//! descriptors, before opening stores or starting worker tasks.
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::TcpListener;

#[derive(Default)]
pub struct Sockets(HashMap<String, Vec<TcpListener>>);

impl Sockets {
    /// Consume fd 3 onwards only for this process. Unknown names and non-TCP
    /// listeners fail startup rather than silently serving the wrong protocol.
    pub fn from_env() -> Result<Self> {
        if std::env::var("LISTEN_PID").ok().as_deref()
            != Some(std::process::id().to_string().as_str())
        {
            return Ok(Self::default());
        }
        let count: i32 = std::env::var("LISTEN_FDS")
            .context("missing LISTEN_FDS")?
            .parse()
            .context("invalid LISTEN_FDS")?;
        if !(0..=1024).contains(&count) {
            bail!("LISTEN_FDS out of range");
        }
        if count == 0 {
            return Ok(Self::default());
        }
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::AcqRel) {
            bail!("activation sockets already consumed");
        }
        for fd in 3..3 + count {
            // SAFETY: fcntl only inspects the number and does not take ownership.
            if unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1 {
                bail!("missing activation descriptor {fd}");
            }
        }
        // Own every passed descriptor immediately, so error paths close them.
        let fds: Vec<OwnedFd> = (3..3 + count)
            .map(|fd| {
                // SAFETY: systemd's process-specific activation contract transfers
                // these descriptors to us; this entry point is called exactly once.
                unsafe { OwnedFd::from_raw_fd(fd) }
            })
            .collect();
        let names = std::env::var("LISTEN_FDNAMES").context("missing LISTEN_FDNAMES")?;
        let names: Vec<_> = names.split(':').collect();
        if names.len() != fds.len() {
            bail!("LISTEN_FDNAMES count mismatch");
        }
        let mut sockets = Self::default();
        for (fd, name) in fds.into_iter().zip(names) {
            if !["smtp", "submissions", "imaps", "jmap"].contains(&name) {
                bail!("unknown maild socket name: {name}");
            }
            use std::os::fd::AsRawFd;
            let raw = fd.as_raw_fd();
            let mut accepting: libc::c_int = 0;
            let mut len = std::mem::size_of_val(&accepting) as libc::socklen_t;
            // SAFETY: getsockopt writes a c_int to the provided sized buffer.
            if unsafe {
                libc::getsockopt(
                    raw,
                    libc::SOL_SOCKET,
                    libc::SO_ACCEPTCONN,
                    (&mut accepting as *mut libc::c_int).cast(),
                    &mut len,
                )
            } != 0
                || accepting != 1
            {
                bail!("{name}: inherited fd is not a listening socket");
            }
            let listener = std::net::TcpListener::from(fd);
            listener
                .local_addr()
                .with_context(|| format!("{name}: expected TCP listener"))?;
            // local_addr alone also accepts Unix sockets on some platforms.
            let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut len = std::mem::size_of_val(&address) as libc::socklen_t;
            // SAFETY: valid socket and sized address buffer.
            if unsafe {
                libc::getsockname(
                    raw,
                    (&mut address as *mut libc::sockaddr_storage).cast(),
                    &mut len,
                )
            } != 0
                || ![libc::AF_INET, libc::AF_INET6].contains(&(address.ss_family as i32))
            {
                bail!("{name}: expected IPv4 or IPv6 TCP listener");
            }
            // Do not leak activation sockets into helper processes.
            // SAFETY: fcntl operates on our live descriptor.
            if unsafe { libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
                return Err(std::io::Error::last_os_error().into());
            }
            listener.set_nonblocking(true)?;
            sockets
                .0
                .entry(name.to_owned())
                .or_default()
                .push(TcpListener::from_std(listener)?);
        }
        Ok(sockets)
    }

    pub fn take(&mut self, name: &str) -> Vec<TcpListener> {
        self.0.remove(name).unwrap_or_default()
    }
}

/// Inherited sockets replace this protocol's configured binds. With none,
/// preserve every configured listener, including ephemeral and multihomed binds.
pub async fn listeners(
    inherited: Vec<TcpListener>,
    addresses: &[String],
) -> Result<Vec<TcpListener>> {
    if !inherited.is_empty() {
        return Ok(inherited);
    }
    let mut listeners = Vec::new();
    for address in addresses {
        listeners.push(
            TcpListener::bind(address)
                .await
                .with_context(|| format!("bind {address}"))?,
        );
    }
    Ok(listeners)
}
