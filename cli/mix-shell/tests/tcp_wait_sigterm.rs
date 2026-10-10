// SPDX-License-Identifier: MIT OR Apache-2.0
//! SIGTERM must end a script waiting in tcp_accept or a TCP receive as
//! promptly as one waiting in sleep(). These waits used to block the
//! single runtime thread, so the shell's SIGTERM select arm never ran and
//! only the 15 s backstop got the process out. The backstop is set far
//! beyond the deadline here, so passing means the graceful path won.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run `script`, wait for it to print "ready", send SIGTERM, and require a
/// graceful 128+SIGTERM exit within `within`.
fn sigterm_ends(name: &str, script: &str, within: Duration) {
    let dir = std::env::temp_dir().join(format!("tcp-sigterm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.mix");
    std::fs::write(&path, script).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_mix"))
        .arg(&path)
        .env("MIX_SIGTERM_BACKSTOP_SECS", "60")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mix");
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "ready", "{name}: the script reached its wait");
    // Give the evaluator time to enter the wait after printing.
    std::thread::sleep(Duration::from_millis(300));
    assert!(child.try_wait().unwrap().is_none(), "{name}: still waiting");
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };

    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if started.elapsed() > within {
            let _ = child.kill();
            let _ = std::fs::remove_dir_all(&dir);
            panic!("{name}: SIGTERM not honoured within {within:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        status.code(),
        Some(128 + libc::SIGTERM),
        "{name}: status {status:?} signal {:?}",
        status.signal()
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("not honoured"), "{name}: stderr {err:?}");
}

const WITHIN: Duration = Duration::from_secs(2);

#[test]
fn sigterm_ends_a_short_timeout_accept_loop() {
    sigterm_ends(
        "accept-loop",
        r#"$l = tcp_listen("127.0.0.1", 0)
print("ready")
while true
  $h = tcp_accept($l, {timeout: 0.05})
end
"#,
        WITHIN,
    );
}

#[test]
fn sigterm_ends_an_accept_with_no_timeout() {
    sigterm_ends(
        "accept-forever",
        r#"$l = tcp_listen("127.0.0.1", 0)
print("ready")
tcp_accept($l, {timeout: 0})
"#,
        WITHIN,
    );
}

#[test]
fn sigterm_ends_a_receive_with_no_timeout() {
    sigterm_ends(
        "recv-line-forever",
        r#"$l = tcp_listen("127.0.0.1", 0)
$a = tcp_local_addr($l)
$c = tcp_connect("127.0.0.1", $a.port, {timeout: 5})
$s = tcp_accept($l, {timeout: 5})
print("ready")
tcp_recv_line($s, {timeout: 0})
"#,
        WITHIN,
    );
}

#[test]
fn sigterm_ends_a_short_timeout_receive_loop() {
    sigterm_ends(
        "recv-loop",
        r#"$l = tcp_listen("127.0.0.1", 0)
$a = tcp_local_addr($l)
$c = tcp_connect("127.0.0.1", $a.port, {timeout: 5})
$s = tcp_accept($l, {timeout: 5})
print("ready")
while true
  $b = tcp_recv($s, {timeout: 0.05})
end
"#,
        WITHIN,
    );
}

/// A send blocked on a peer that never reads (payload larger than the
/// socket buffers, no deadline) ends on SIGTERM too.
#[test]
fn sigterm_ends_a_send_blocked_on_a_non_reading_peer() {
    sigterm_ends(
        "send-blocked",
        r#"$l = tcp_listen("127.0.0.1", 0)
$a = tcp_local_addr($l)
$c = tcp_connect("127.0.0.1", $a.port, {timeout: 5})
$s = tcp_accept($l, {timeout: 5})
$payload = repeat("x", 67108864)
print("ready")
tcp_send($c, $payload, {timeout: 0})
"#,
        WITHIN,
    );
}
