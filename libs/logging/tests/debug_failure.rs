// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fatal stderr delivery is independent of the terminal, filter and journal.
use std::fs::File;
use std::io::Read;
use std::os::fd::FromRawFd;
use std::process::{Command, Stdio};

const CHILD: &str = "MIXOS_FATAL_FIXTURE";
const ERROR: &str = "service already registered";

#[test]
fn fatal_errors_print_once_and_reach_enabled_sinks() {
    if let Ok(app) = std::env::var(CHILD) {
        let level = std::env::var("MIXOS_FATAL_LEVEL").unwrap();
        let mut args = vec![app.clone(), format!("--debug={level}")];
        let _guard = logging::debug::init(&app, "warn", &mut args).unwrap();
        logging::debug::failure(&app, &ERROR);
        let (rc, recent) =
            logging::debug::request(&app, &format!("{app}.debug.recent"), "{}").unwrap();
        assert_eq!(rc, 0);
        let failures: Vec<_> = recent["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["fields"]["error"] == ERROR)
            .collect();
        if level == "debug" {
            assert_eq!(failures.len(), 1, "{recent}");
            assert_eq!(failures[0]["level"], "error");
            assert_eq!(failures[0]["fields"]["app"], app);
            assert_eq!(failures[0]["message"], "application error");
        } else {
            assert!(failures.is_empty(), "{recent}");
        }
        return;
    }

    // Probe the real sink. Hosts without journald still exercise stderr fallback.
    let journal = tracing_journald::layer().is_ok();
    for tty in [false, true] {
        for level in ["error", "debug", "off"] {
            let app = format!("fatal-fixture-{}-{tty}-{level}", std::process::id());
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "fatal_errors_print_once_and_reach_enabled_sinks",
                    "--nocapture",
                ])
                .env(CHILD, &app)
                .env("MIXOS_FATAL_LEVEL", level)
                .env_remove("RUST_LOG")
                .stdout(Stdio::null());
            let stderr = if tty {
                let (mut master, mut slave) = (-1, -1);
                // SAFETY: valid fd pointers; null opts request default tty settings.
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
                    0
                );
                // SAFETY: each owned openpty fd is transferred exactly once.
                let (mut master, slave) =
                    unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
                let status = command.stderr(slave).status().unwrap();
                // Drop Command's slave handle so read_to_end observes EOF/EIO.
                drop(command);
                assert!(status.success(), "{app}: {status}");
                let mut bytes = Vec::new();
                if let Err(error) = master.read_to_end(&mut bytes) {
                    assert_eq!(error.raw_os_error(), Some(libc::EIO));
                }
                String::from_utf8(bytes).unwrap().replace("\r\n", "\n")
            } else {
                let output = command.output().unwrap();
                assert!(output.status.success(), "{app}: {output:?}");
                String::from_utf8(output.stderr).unwrap()
            };
            let expected = format!("{app}: {ERROR}");
            // Debug startup messages are allowed; the fatal error is exactly one
            // plain line, with no duplicate formatted 'application error' event.
            assert_eq!(stderr.matches(ERROR).count(), 1, "{stderr}");
            assert_eq!(
                stderr.lines().filter(|line| *line == expected).count(),
                1,
                "{stderr}"
            );
            assert!(!stderr.contains("application error"), "{stderr}");
            if level != "debug" {
                assert_eq!(stderr, format!("{expected}\n"));
            }
            if journal {
                assert_journal(&app, level != "off");
            }
        }
    }
}

fn assert_journal(app: &str, enabled: bool) {
    // Journald ingests asynchronously. Poll by unique identity, without touching
    // global journal state or depending on messages from other tests.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let output = Command::new("journalctl")
            .args(["--no-pager", "-o", "json", "-t", app, "PRIORITY=3"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let records: Vec<serde_json::Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        if !enabled {
            assert!(records.is_empty(), "{records:?}");
            return;
        }
        if !records.is_empty() {
            assert_eq!(records.len(), 1, "{records:?}");
            assert_eq!(records[0]["F_APP"], app);
            assert_eq!(records[0]["F_ERROR"], ERROR);
            assert_eq!(records[0]["MESSAGE"], "application error");
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "missing journal record for {app}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
