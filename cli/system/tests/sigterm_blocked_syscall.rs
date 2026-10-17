// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! SIGTERM must end a non-interactive mix even when evaluation is stuck in a
//! synchronous syscall. tokio's handler replaces the default disposition and
//! only wakes a `select!` arm, which a builtin blocked on the single runtime
//! thread never lets run: `append_file` to a FIFO with no reader survived
//! SIGTERM for a week (2026-10-02). The backstop thread must force the exit.
//!
//! `read_file`, `write_file` and `append_file` no longer block on a FIFO
//! (they wait without holding the thread, so SIGTERM is graceful — the
//! second half of this file). The backstop test uses `read_file_bytes`,
//! which still opens synchronously.

#![cfg(target_os = "linux")]

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn sigterm_backstop_ends_a_script_blocked_in_a_syscall() {
    let dir = std::env::temp_dir().join(format!("sigterm-blocked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("f");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
    let script = dir.join("t.mix");
    std::fs::write(
        &script,
        format!(
            "read_file_bytes(\"{}\")\nprint(\"unreachable\")\n",
            fifo.display()
        ),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_mix"))
        .arg(&script)
        .env("MIX_SIGTERM_BACKSTOP_SECS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mix");

    // Let it reach the blocking open.
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        child.try_wait().unwrap().is_none(),
        "script should be blocked on the FIFO"
    );
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };

    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = std::fs::remove_dir_all(&dir);
            panic!("mix ignored SIGTERM while blocked in a syscall");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        status.code(),
        Some(128 + libc::SIGTERM),
        "status {status:?} signal {:?}",
        status.signal()
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("SIGTERM not honoured"), "stderr: {err:?}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("unreachable"));
}

/// TODO-mix 2026-10-02: the FIFO repro gets GRACEFUL shutdown on SIGTERM.
/// The backstop is set far out of reach, so only the ordinary shutdown arm
/// can end the script in time, and the backstop's message must not appear.
#[test]
fn sigterm_is_graceful_while_a_file_builtin_waits_on_a_fifo() {
    for (tag, call) in [
        ("append", "append_file(\"{}\", \"hi\\n\")"),
        ("write", "write_file(\"{}\", \"hi\\n\")"),
        ("read", "read_file(\"{}\")"),
    ] {
        let dir = std::env::temp_dir().join(format!("sigterm-fifo-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("f");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
        let script = dir.join("t.mix");
        std::fs::write(
            &script,
            format!(
                "{}\nprint(\"unreachable\")\n",
                call.replace("{}", fifo.to_str().unwrap())
            ),
        )
        .unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_mix"))
            .arg(&script)
            .env("MIX_SIGTERM_BACKSTOP_SECS", "60")
            .env("MIX_STATS", "off")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn mix");
        std::thread::sleep(Duration::from_millis(800));
        assert!(
            child.try_wait().unwrap().is_none(),
            "{tag}: script should be waiting on the FIFO"
        );
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let started = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait().unwrap() {
                break s;
            }
            if started.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = std::fs::remove_dir_all(&dir);
                panic!("{tag}: SIGTERM was not graceful while waiting on a FIFO");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let out = child.wait_with_output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            status.code(),
            Some(128 + libc::SIGTERM),
            "{tag}: {status:?}"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            !err.contains("SIGTERM not honoured"),
            "{tag}: stderr: {err:?}"
        );
        assert!(!String::from_utf8_lossy(&out.stdout).contains("unreachable"));
    }
}

/// The FIFO path still moves the data, both ways, while the other end is a
/// separate process.
#[test]
fn file_builtins_still_read_and_write_a_fifo() {
    let dir = std::env::temp_dir().join(format!("fifo-roundtrip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("f");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
    let path = fifo.to_str().unwrap().to_string();
    // A writer process feeds the reader script.
    let writer = Command::new(env!("CARGO_BIN_EXE_mix"))
        .args(["-c", &format!("write_file(\"{path}\", \"ping\")")])
        .env("MIX_STATS", "off")
        .spawn()
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mix"))
        .args(["-c", &format!("print(\"got=\" .. read_file(\"{path}\"))")])
        .env("MIX_STATS", "off")
        .output()
        .unwrap();
    let _ = writer.wait_with_output();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "got=ping");
}
