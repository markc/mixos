// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exercise real inherited fds in an isolated process, never the test runner's
//! environment or fd table. The child sets LISTEN_PID before starting Tokio.
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;

#[test]
fn named_fd_adoption_and_fallback() {
    let names = ["jmap", "smtp", "smtp", "submissions", "imaps"];
    let sockets: Vec<_> = names.iter().map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap()).collect();
    // Keep sources above the entire target range so dup2 cannot clobber a
    // source we have not yet copied (libtest may already hold fd 3).
    let fds: Vec<OwnedFd> = sockets.iter().map(|s| {
        let fd = unsafe { libc::fcntl(s.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) };
        assert!(fd >= 64);
        unsafe { OwnedFd::from_raw_fd(fd) }
    }).collect();
    for (mode, expected) in [("adopt", true), ("wrong_pid", true), ("unknown", false), ("bad_count", false)] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", "activation_child", "--nocapture"])
            .env("MAILD_ACTIVATION_TEST", mode)
            .env("LISTEN_FDS", names.len().to_string())
            .env("LISTEN_FDNAMES", if mode == "unknown" { "unknown:smtp:smtp:submissions:imaps" } else if mode == "bad_count" { "smtp" } else { "jmap:smtp:smtp:submissions:imaps" })
            .env("ACTIVATED_ADDRS", sockets.iter().map(|s| s.local_addr().unwrap().to_string()).collect::<Vec<_>>().join(","));
        let raw: Vec<_> = fds.iter().map(AsRawFd::as_raw_fd).collect();
        unsafe {
            child.pre_exec(move || {
                for (index, fd) in raw.iter().enumerate() {
                    if libc::dup2(*fd, 3 + index as i32) < 0 { return Err(std::io::Error::last_os_error()); }
                }
                Ok(())
            });
        }
        let output = child.output().unwrap();
        assert_eq!(output.status.success(), expected, "{mode}: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
    drop(fds);
}

#[test]
fn activation_child() {
    let Ok(mode) = std::env::var("MAILD_ACTIVATION_TEST") else { return; };
    // This isolated helper has no other environment readers or worker tasks.
    unsafe { std::env::set_var("LISTEN_PID", if mode == "wrong_pid" { "0".into() } else { std::process::id().to_string() }); }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let mut sockets = maild::activation::Sockets::from_env().unwrap();
        let addresses: Vec<_> = std::env::var("ACTIVATED_ADDRS").unwrap().split(',').map(str::to_owned).collect();
        if mode == "wrong_pid" {
            assert!(sockets.take("smtp").is_empty());
            return;
        }
        for (name, indices) in [("jmap", vec![0]), ("smtp", vec![1, 2]), ("submissions", vec![3]), ("imaps", vec![4])] {
            let listeners = maild::activation::listeners(sockets.take(name), &["invalid-bind".into()]).await.unwrap();
            assert_eq!(listeners.len(), indices.len());
            for (listener, index) in listeners.into_iter().zip(indices) {
                assert_eq!(listener.local_addr().unwrap().to_string(), addresses[index]);
                let addr = listener.local_addr().unwrap();
                let connect = tokio::spawn(tokio::net::TcpStream::connect(addr));
                listener.accept().await.unwrap();
                connect.await.unwrap().unwrap();
                assert!(unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC != 0);
            }
            assert!(sockets.take(name).is_empty());
        }
        let fallback = maild::activation::listeners(Vec::new(), &["127.0.0.1:0".into(), "127.0.0.1:0".into()]).await.unwrap();
        assert_eq!(fallback.len(), 2);
    });
}
