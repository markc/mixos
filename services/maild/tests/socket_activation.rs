// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exercise real inherited fds in an isolated process, never the test runner's
//! environment or fd table. The child sets LISTEN_PID before starting Tokio.
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;

#[test]
fn named_fd_adoption_and_fallback() {
    let names = ["jmap", "smtp", "smtp", "submissions", "imaps"];
    let sockets: Vec<_> = names
        .iter()
        .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    // Keep sources above the entire target range so dup2 cannot clobber a
    // source we have not yet copied (libtest may already hold fd 3).
    let fds: Vec<OwnedFd> = sockets
        .iter()
        .map(|s| {
            let fd = unsafe { libc::fcntl(s.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) };
            assert!(fd >= 64);
            unsafe { OwnedFd::from_raw_fd(fd) }
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let unix = std::os::unix::net::UnixListener::bind(dir.path().join("socket")).unwrap();
    let (connected, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let invalid: Vec<OwnedFd> = [unix.as_raw_fd(), connected.as_raw_fd()].into_iter().map(|raw| {
        let fd = unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 64) };
        assert!(fd >= 64);
        unsafe { OwnedFd::from_raw_fd(fd) }
    }).collect();
    for (mode, expected) in [
        ("adopt", true),
        ("runtime", true),
        ("wrong_pid", true),
        ("unknown", false),
        ("bad_count", false),
        ("unix_listener", false),
        ("not_listener", false),
    ] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "activation_child", "--nocapture"])
            .env("MAILD_ACTIVATION_TEST", mode)
            .env("LISTEN_FDS", names.len().to_string())
            .env(
                "LISTEN_FDNAMES",
                if mode == "unknown" {
                    "unknown:smtp:smtp:submissions:imaps"
                } else if mode == "bad_count" {
                    "smtp"
                } else {
                    "jmap:smtp:smtp:submissions:imaps"
                },
            )
            .env(
                "ACTIVATED_ADDRS",
                sockets
                    .iter()
                    .map(|s| s.local_addr().unwrap().to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        let mut raw: Vec<_> = fds.iter().map(AsRawFd::as_raw_fd).collect();
        if mode == "unix_listener" { raw[0] = invalid[0].as_raw_fd(); }
        if mode == "not_listener" { raw[0] = invalid[1].as_raw_fd(); }
        unsafe {
            child.pre_exec(move || {
                for (index, fd) in raw.iter().enumerate() {
                    if libc::dup2(*fd, 3 + index as i32) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let output = child.output().unwrap();
        assert_eq!(
            output.status.success(),
            expected,
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    drop(fds);
}

#[test]
fn activation_child() {
    let Ok(mode) = std::env::var("MAILD_ACTIVATION_TEST") else {
        return;
    };
    // This isolated helper has no other environment readers or worker tasks.
    unsafe {
        std::env::set_var(
            "LISTEN_PID",
            if mode == "wrong_pid" {
                "0".into()
            } else {
                std::process::id().to_string()
            },
        );
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut sockets = maild::activation::Sockets::from_env().unwrap();
        let addresses: Vec<_> = std::env::var("ACTIVATED_ADDRS")
            .unwrap()
            .split(',')
            .map(str::to_owned)
            .collect();
        if mode == "wrong_pid" {
            assert!(sockets.take("smtp").is_empty());
            return;
        }
        assert!(maild::activation::Sockets::from_env().is_err());
        if mode == "runtime" {
            use maild::{config::{Config, ListenSpec}, runtime::{RuntimeOpts, build_runtime_with_sockets}};
            let dir = tempfile::tempdir().unwrap();
            let path = |name| dir.path().join(name).to_str().unwrap().to_owned();
            let rcgen::CertifiedKey { cert, key_pair } = rcgen::generate_simple_self_signed(vec!["example.com".into()]).unwrap();
            std::fs::write(path("cert.pem"), cert.pem()).unwrap();
            std::fs::write(path("key.pem"), key_pair.serialize_pem()).unwrap();
            let cfg = Config {
                database_path: path("mail.db"), blob_dir: path("blobs"), mds_dir: path("mds"),
                spam_db_dir: Some(path("spam")), rule_stats_dir: Some(path("rules")),
                tls_cert: Some(path("cert.pem")), tls_key: Some(path("key.pem")),
                smtp_inbound: Some(ListenSpec::Single("invalid-bind".into())),
                smtp_smtps: Some(ListenSpec::Single("invalid-bind".into())),
                imap_imaps: Some(ListenSpec::Single("invalid-bind".into())),
                ..Default::default()
            };
            let built = build_runtime_with_sockets(&cfg, RuntimeOpts { enable_bus: false, disable_outbound_delivery: true, ..Default::default() }, &mut sockets).await.unwrap();
            assert_eq!(built.smtp_handle.inbound_addrs.iter().map(ToString::to_string).collect::<Vec<_>>(), addresses[1..3]);
            assert_eq!(built.smtp_handle.smtps_addrs[0].to_string(), addresses[3]);
            assert_eq!(built.imap_handle.imaps_addrs[0].to_string(), addresses[4]);
            use tokio::io::AsyncBufReadExt;
            let stream = tokio::net::TcpStream::connect(&addresses[1]).await.unwrap();
            let mut greeting = String::new();
            tokio::time::timeout(std::time::Duration::from_secs(5), tokio::io::BufReader::new(stream).read_line(&mut greeting)).await.unwrap().unwrap();
            assert!(greeting.starts_with("220 "), "{greeting}");
            let listeners = maild::activation::listeners(sockets.take("jmap"), &["invalid-bind".into()]).await.unwrap();
            let app = built.router.clone();
            let listener = listeners.into_iter().next().unwrap();
            let serve = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
            let status = reqwest::get(format!("http://{}/.well-known/jmap", addresses[0])).await.unwrap().status();
            assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
            serve.abort();
            return;
        }
        for (name, indices) in [
            ("jmap", vec![0]),
            ("smtp", vec![1, 2]),
            ("submissions", vec![3]),
            ("imaps", vec![4]),
        ] {
            let listeners =
                maild::activation::listeners(sockets.take(name), &["invalid-bind".into()])
                    .await
                    .unwrap();
            assert_eq!(listeners.len(), indices.len());
            for (listener, index) in listeners.into_iter().zip(indices) {
                assert_eq!(listener.local_addr().unwrap().to_string(), addresses[index]);
                let addr = listener.local_addr().unwrap();
                let connect = tokio::spawn(tokio::net::TcpStream::connect(addr));
                listener.accept().await.unwrap();
                connect.await.unwrap().unwrap();
                assert!(
                    unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC
                        != 0
                );
            }
            assert!(sockets.take(name).is_empty());
        }
        let fallback =
            maild::activation::listeners(Vec::new(), &["127.0.0.1:0".into(), "127.0.0.1:0".into()])
                .await
                .unwrap();
        assert_eq!(fallback.len(), 2);
    });
}
