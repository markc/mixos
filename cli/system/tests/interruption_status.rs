// SPDX-License-Identifier: MIT OR Apache-2.0
//! Error prose never grants interruption status; real signals retain CLI exits.
#![cfg(unix)]

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn command(home: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mix"));
    cmd.env("HOME", home)
        .env("MIX_STATS", "off")
        .env("MIX_SIGTERM_BACKSTOP_SECS", "1")
        .env_remove("COSMIX_SESSION_FD")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

#[test]
fn error_messages_and_codes_cannot_impersonate_a_signal() {
    let dir = tempfile::tempdir().unwrap();
    for (body, diagnostic) in [
        (
            r#"raise("PROBE_REFUSAL", json_encode({interrupted:false,error:"not a signal"}))"#,
            "PROBE_REFUSAL",
        ),
        (
            r#"die("ordinary interrupted operation")"#,
            "ordinary interrupted operation",
        ),
        (r#"run_argv_must(["false"])"#, "PROCESS_EXIT_NONZERO"),
        (r#"raise("SIGNAL_INTERRUPT", "2")"#, "SIGNAL_INTERRUPT"),
    ] {
        for mode in ["command", "file", "function"] {
            let mut cmd = command(dir.path());
            match mode {
                "command" => {
                    cmd.args(["-c", body]);
                }
                "file" => {
                    let script = dir.path().join("probe.mix");
                    std::fs::write(&script, body).unwrap();
                    cmd.arg(script);
                }
                _ => {
                    std::fs::write(
                        dir.path().join(".mixrc"),
                        format!("fn probe_fail()\n{body}\nend\n"),
                    )
                    .unwrap();
                    cmd.args(["-i", "-c", "probe_fail"]);
                }
            }
            let out = cmd.output().unwrap();
            let err = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(1), "{mode}: {err}");
            assert!(
                err.contains(diagnostic),
                "{mode}: missing {diagnostic}: {err}"
            );
        }
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn assert_sigint_error_frame(path: &std::path::Path) {
    let frame = std::fs::read(path).unwrap();
    assert!(frame.len() >= 4, "missing result frame: {frame:?}");
    let declared = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    assert_eq!(declared, frame.len() - 4, "torn result frame");
    let text = std::str::from_utf8(&frame[4..]).unwrap();
    let parsed = mix::parse_data(text).unwrap();
    let mix::value::Value::Map(payload) = &parsed else {
        panic!("result frame must be a map: {text}");
    };
    assert!(
        matches!(payload.get("ok"), Some(mix::value::Value::Bool(false))),
        "interrupted execution emitted a success frame: {text}"
    );
    assert!(
        matches!(payload.get("error"), Some(mix::value::Value::String(error))
            if error.contains("signal 2")),
        "result frame did not report SIGINT: {text}"
    );
    assert!(!payload.contains_key("value"), "unexpected value: {text}");
}

fn assert_caught_sigint(mode: &str, framed: bool) {
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let caught = dir.path().join("caught");
    // No await that yields to the runtime: the catch returns a successful
    // value in the same poll that consumes the evaluator's interrupt flag.
    let body = format!(
        "try\nwrite_file({}, \"ready\")\n$n=0\nwhile true\n$n=$n+1\nend\n\
         catch $err\nwrite_file({}, \"caught\")\nreturn 42\nend\n",
        serde_json::to_string(ready.to_str().unwrap()).unwrap(),
        serde_json::to_string(caught.to_str().unwrap()).unwrap(),
    );
    let result_path = dir.path().join("result");
    let result_file = framed.then(|| std::fs::File::create(&result_path).unwrap());
    let mut cmd = command(dir.path());
    cmd.arg("--no-prelude");
    if let Some(file) = &result_file {
        let raw = file.as_raw_fd();
        // This must precede -c, which consumes the remaining CLI args.
        cmd.args(["--result-fd", "64"]);
        // SAFETY: only async-signal-safe descriptor operations run in the
        // child; result_file owns the source descriptor through spawn.
        unsafe {
            cmd.pre_exec(move || {
                if libc::dup2(raw, 64) < 0 || libc::fcntl(64, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    match mode {
        "command" => {
            cmd.args(["-c", &body]);
        }
        "file" => {
            let script = dir.path().join("caught_signal.mix");
            std::fs::write(&script, &body).unwrap();
            cmd.arg(script);
        }
        "function" => {
            std::fs::write(
                dir.path().join(".mixrc"),
                format!("fn probe_catch()\n{body}\nend\n"),
            )
            .unwrap();
            cmd.args(["-i", "-c", "probe_catch"]);
        }
        _ => panic!("unknown mode: {mode}"),
    }
    let mut child = OwnedChild(cmd.spawn().unwrap());
    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while !ready.is_file() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "{mode} exited before ready (framed={framed})"
        );
        assert!(
            Instant::now() < ready_deadline,
            "{mode} did not reach the try body (framed={framed})"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGINT) }, 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "{mode} ignored SIGINT (framed={framed})"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(130), "{mode} framed={framed}: {status}");
    assert!(caught.is_file(), "{mode} never caught the interrupt");
    if framed {
        assert_sigint_error_frame(&result_path);
    }
}

#[test]
fn caught_sigint_exits_130_for_command() {
    assert_caught_sigint("command", false);
}

#[test]
fn caught_sigint_exits_130_for_script_file() {
    assert_caught_sigint("file", false);
}

#[test]
fn caught_sigint_exits_130_for_function_command() {
    assert_caught_sigint("function", false);
}

#[test]
fn caught_sigint_exits_130_and_emits_error_frame_for_result_fd() {
    assert_caught_sigint("command", true);
}

#[test]
fn real_signals_exit_nonzero_for_waiting_and_cooperative_evaluations() {
    for mode in ["command", "file", "function"] {
        for work in ["sleep(30)", "$n=0\nwhile true\n$n=$n+1\nend"] {
            for signal in [libc::SIGINT, libc::SIGTERM] {
                let dir = tempfile::tempdir().unwrap();
                let marker = dir.path().join("ready");
                let body = format!(
                    "write_file({}, \"ready\")\n{work}\nprint(\"UNREACHABLE\")\n",
                    serde_json::to_string(marker.to_str().unwrap()).unwrap()
                );
                let mut cmd = command(dir.path());
                match mode {
                    "command" => {
                        cmd.args(["-c", &body]);
                    }
                    "file" => {
                        let script = dir.path().join("signal.mix");
                        std::fs::write(&script, &body).unwrap();
                        cmd.arg(script);
                    }
                    _ => {
                        std::fs::write(
                            dir.path().join(".mixrc"),
                            format!("fn probe_run()\n{body}\nend\n"),
                        )
                        .unwrap();
                        cmd.args(["-i", "-c", "probe_run"]);
                    }
                }
                let mut child = OwnedChild(cmd.spawn().unwrap());
                let ready_deadline = Instant::now() + Duration::from_secs(5);
                while !marker.is_file() {
                    assert!(
                        child.0.try_wait().unwrap().is_none(),
                        "{mode} exited before ready"
                    );
                    assert!(
                        Instant::now() < ready_deadline,
                        "{mode} did not reach evaluation"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert!(child.0.try_wait().unwrap().is_none());
                assert_eq!(unsafe { libc::kill(child.0.id() as i32, signal) }, 0);
                let deadline = Instant::now() + Duration::from_secs(20);
                let status = loop {
                    if let Some(status) = child.0.try_wait().unwrap() {
                        break status;
                    }
                    assert!(Instant::now() < deadline, "{mode} ignored signal {signal}");
                    std::thread::sleep(Duration::from_millis(10));
                };
                assert_eq!(
                    status.code(),
                    Some(128 + signal),
                    "{mode} {work} signal {signal}: {status}"
                );
            }
        }
    }
}

#[test]
fn sigint_during_cpu_only_prelude_exits_with_130_for_command_and_file() {
    for mode in ["command", "file"] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("prelude.mix"),
            "print(\"PRELUDE_READY\")\n$n=0\nwhile true\n$n=$n+1\nend\n",
        )
        .unwrap();
        let stdout_path = dir.path().join("stdout");
        let mut cmd = command(dir.path());
        cmd.env("MIXOS_ETC", dir.path())
            .stdout(Stdio::from(std::fs::File::create(&stdout_path).unwrap()));
        match mode {
            "command" => {
                cmd.args(["-c", "print(\"UNREACHABLE\")"]);
            }
            _ => {
                let script = dir.path().join("signal.mix");
                std::fs::write(&script, "print(\"UNREACHABLE\")\n").unwrap();
                cmd.arg(script);
            }
        }
        let mut child = OwnedChild(cmd.spawn().unwrap());
        let ready_deadline = Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(&stdout_path)
            .unwrap()
            .contains("PRELUDE_READY")
        {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "{mode} exited before reaching the prelude loop"
            );
            assert!(
                Instant::now() < ready_deadline,
                "{mode} prelude never ready"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(child.0.try_wait().unwrap().is_none());
        assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGINT) }, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "{mode} prelude ignored SIGINT");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(130), "{mode}: {status}");
        assert!(
            !std::fs::read_to_string(&stdout_path)
                .unwrap()
                .contains("UNREACHABLE"),
            "{mode} ran the command after its prelude was interrupted"
        );
    }
}

fn assert_framed_startup_sigint(startup: &str) {
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let executed = dir.path().join("executed");
    let body = format!(
        "write_file({}, \"ready\")\n$n=0\nwhile true\n$n=$n+1\nend\n",
        serde_json::to_string(ready.to_str().unwrap()).unwrap(),
    );
    let mut cmd = command(dir.path());
    match startup {
        "prelude" => {
            std::fs::write(dir.path().join("prelude.mix"), &body).unwrap();
            cmd.env("MIXOS_ETC", dir.path());
        }
        "mixrc" => {
            std::fs::write(dir.path().join(".mixrc"), &body).unwrap();
            cmd.args(["--no-prelude", "-i"]);
        }
        _ => panic!("unknown startup: {startup}"),
    }

    // Use the same PTY stdin pattern as process_lifetime: terminal stdin is
    // essential to exercise the historical unframed SIGINT exit-0 carve-out.
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: openpty writes two fresh descriptors into these locals.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0,
        "openpty failed"
    );
    // SAFETY: these descriptors are fresh and owned here. Keep the master
    // alive until the child exits so its stdin remains a live terminal.
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
    }
    assert_eq!(unsafe { libc::isatty(slave.as_raw_fd()) }, 1);
    cmd.stdin(Stdio::from(slave));

    let result_path = dir.path().join("result");
    let result_file = std::fs::File::create(&result_path).unwrap();
    let raw = result_file.as_raw_fd();
    cmd.args(["--result-fd", "64", "-c"]);
    cmd.arg(format!(
        "write_file({}, \"executed\")",
        serde_json::to_string(executed.to_str().unwrap()).unwrap(),
    ));
    // SAFETY: only async-signal-safe descriptor operations run in the child;
    // result_file owns the source descriptor through spawn.
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(raw, 64) < 0 || libc::fcntl(64, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = OwnedChild(cmd.spawn().unwrap());
    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while !ready.is_file() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "{startup} exited before reaching the startup loop"
        );
        assert!(Instant::now() < ready_deadline, "{startup} never ready");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(unsafe { libc::kill(child.0.id() as i32, libc::SIGINT) }, 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "{startup} ignored SIGINT");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(130), "{startup}: {status}");
    assert_sigint_error_frame(&result_path);
    assert!(
        !executed.exists(),
        "command ran after {startup} was interrupted"
    );
}

#[test]
fn framed_sigint_during_prelude_exits_130_and_emits_error_frame_on_terminal_stdin() {
    assert_framed_startup_sigint("prelude");
}

#[test]
fn framed_sigint_during_mixrc_exits_130_and_emits_error_frame_on_terminal_stdin() {
    assert_framed_startup_sigint("mixrc");
}
