// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real children dispatched through the ordinary native event pump.
#![cfg(target_os = "linux")]
use mix::{MixResult, evaluator::Evaluator, lexer::Lexer, parser::Parser, value::Value};
use std::time::Duration;

async fn exec(e: &mut Evaluator, source: &str) -> MixResult<Value> {
    let tokens = Lexer::new(source).tokenize()?;
    e.execute(&Parser::new(tokens, source).parse_program()?).await
}

async fn pump(e: &mut Evaluator) {
    tokio::time::timeout(Duration::from_secs(10), e.run_event_pump()).await.unwrap().unwrap();
}

async fn watcher() -> Evaluator {
    let mut e = Evaluator::new();
    exec(&mut e, r#"
        $lines = []
        $exits = []
        on proc.line
            $lines = concat($lines, [$event.args])
        end
        on proc.watch_exited
            $exits = concat($exits, [$event.args])
        end
    "#).await.unwrap();
    e
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_lines_then_exactly_one_terminal_in_plain_script() {
    let mut e = watcher().await;
    exec(&mut e, r#"$h = process_watch(["seq", "1", "20"], {tag: "ordered"})"#).await.unwrap();
    pump(&mut e).await;
    let Value::List(lines) = e.get_global("lines").unwrap() else { panic!() };
    assert_eq!(lines.len(), 20);
    for (n, line) in lines.iter().enumerate() {
        let Value::Map(m) = line else { panic!() };
        assert_eq!(m["line"], Value::String((n + 1).to_string()));
        assert_eq!(m["tag"], Value::String("ordered".into()));
        assert_eq!(m["stream"], Value::String("stdout".into()));
        assert_eq!(m["truncated"], Value::Bool(false));
    }
    let Value::List(exits) = e.get_global("exits").unwrap() else { panic!() };
    assert_eq!(exits.len(), 1);
    let Value::Map(m) = &exits[0] else { panic!() };
    assert_eq!(m["exit_code"], Value::Number(0.0));
    pump(&mut e).await;
    assert_eq!(e.get_global("exits").unwrap(), Value::List(exits));
}

#[tokio::test(flavor = "current_thread")]
async fn truncates_and_flushes_unterminated_line() {
    let mut e = watcher().await;
    exec(&mut e, r#"$h = process_watch(["printf", "abcdef\nxyz"], {max_line: 3})"#).await.unwrap();
    pump(&mut e).await;
    let text = e.get_global("lines").unwrap().to_mix_string();
    assert!(text.contains("line: abc, truncated: true"), "{text}");
    assert!(text.contains("line: xyz, truncated: false"), "{text}");
}

#[tokio::test(flavor = "current_thread")]
async fn stderr_modes_and_shared_environment_validation() {
    for mode in ["null", "lines", "inherit"] {
        let mut e = watcher().await;
        e.set_global("mode", Value::String(mode.into()));
        // env's invalid option produces stderr without requiring any shell.
        exec(&mut e, r#"$h = process_watch(["/usr/bin/env", "--mix-no-such-option"], {stderr: $mode, clear_env: true, env: {LC_ALL: "C"}})"#).await.unwrap();
        pump(&mut e).await;
        let Value::List(lines) = e.get_global("lines").unwrap() else { panic!() };
        assert_eq!(!lines.is_empty(), mode == "lines");
        for line in lines.iter() {
            let Value::Map(m) = line else { panic!() };
            assert_eq!(m["stream"], Value::String("stderr".into()));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn unwatch_and_generation_shutdown_reap_sleep() {
    for unwatch in [true, false] {
        let mut e = watcher().await;
        // Read the owned PID through /proc rather than exposing one in the API.
        exec(&mut e, r#"$h = process_watch(["sleep", "1000"])
            $h2 = process_watch(["sleep", "1000"])
        "#).await.unwrap();
        let children = std::fs::read_to_string(format!("/proc/self/task/{}/children", unsafe { libc::syscall(libc::SYS_gettid) })).unwrap();
        let pids: Vec<i32> = children.split_whitespace().map(|s| s.parse().unwrap()).collect();
        assert!(pids.len() >= 2);
        if unwatch {
            assert_eq!(exec(&mut e, "process_unwatch($h)").await.unwrap(), Value::Bool(true));
            assert_eq!(exec(&mut e, "process_unwatch($h)").await.unwrap(), Value::Bool(false));
        }
        e.close_native_events();
        for pid in pids {
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "leftover {pid}");
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) }, -1);
            assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ECHILD));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn reload_candidate_and_bad_options_never_start_children() {
    let mut e = Evaluator::new();
    e.set_reload_candidate(true);
    let error = exec(&mut e, r#"process_watch(["sleep", "1000"])"#).await.unwrap_err();
    assert!(error.to_string().contains("PROC_WATCH_RELOAD_CANDIDATE"));
    e.set_reload_candidate(false);
    for source in [
        r#"process_watch([])"#, r#"process_watch(["sleep", 1])"#,
        r#"process_watch(["true"], {max_queue: 0})"#,
        r#"process_watch(["true"], {max_line: 1.5})"#,
        r#"process_watch(["true"], {stderr: "capture"})"#,
        r#"process_watch(["true"], {env: {"bad=name": "x"}})"#,
        r#"process_watch(["true"], {detach: true})"#,
    ] { assert!(exec(&mut e, source).await.is_err(), "{source}"); }
}
