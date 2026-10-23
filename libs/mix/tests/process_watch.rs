// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real children dispatched through the ordinary native event pump.
#![cfg(target_os = "linux")]
use mix::{MixResult, evaluator::Evaluator, lexer::Lexer, parser::Parser, value::Value};
use std::time::Duration;

async fn exec(e: &mut Evaluator, source: &str) -> MixResult<Value> {
    let tokens = Lexer::new(source).tokenize()?;
    e.execute(&Parser::new(tokens, source).parse_program()?)
        .await
}

async fn pump(e: &mut Evaluator) {
    tokio::time::timeout(Duration::from_secs(10), e.run_event_pump())
        .await
        .unwrap()
        .unwrap();
}

async fn watcher() -> Evaluator {
    let mut e = Evaluator::new();
    exec(
        &mut e,
        r#"
        $lines = []
        $exits = []
        $last_line_count = 0
        $pong = false
        on proc.line
            $lines = concat($lines, [$event.args])
        end
        on proc.watch_exited
            $last_line_count = len($lines)
            $exits = concat($exits, [$event.args])
        end
    "#,
    )
    .await
    .unwrap();
    e
}

#[tokio::test(flavor = "current_thread")]
async fn no_handlers_drains_and_finished_unwatch_is_false() {
    let mut e = Evaluator::new();
    exec(&mut e, r#"$h = process_watch(["true"])"#).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), e.run_event_pump()).await.unwrap().unwrap();
    assert_eq!(exec(&mut e, "process_unwatch($h)").await.unwrap(), Value::Bool(false));
}

#[tokio::test(flavor = "current_thread")]
async fn finished_but_undelivered_unwatch_is_true() {
    let mut e = watcher().await;
    exec(&mut e, r#"$h = process_watch(["true"])"#).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(exec(&mut e, "process_unwatch($h)").await.unwrap(), Value::Bool(true));
    pump(&mut e).await;
    assert_eq!(exec(&mut e, "len($exits)").await.unwrap(), Value::Number(0.0));
}

#[tokio::test(flavor = "current_thread")]
async fn term_exit_does_not_wait_out_grace() {
    let mut e = watcher().await;
    exec(&mut e, r#"$h = process_watch(["sleep", "30"])"#).await.unwrap();
    let start = std::time::Instant::now();
    assert_eq!(exec(&mut e, "process_unwatch($h)").await.unwrap(), Value::Bool(true));
    assert!(start.elapsed() < Duration::from_millis(100), "unwatch took {:?}", start.elapsed());
}

#[tokio::test(flavor = "current_thread")]
async fn detached_pipe_holder_has_bounded_drain() {
    struct Cleanup(i32);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            unsafe { libc::kill(self.0, libc::SIGKILL); }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("grandchild.pid");
    let mut e = watcher().await;
    e.set_global("command", Value::String(format!("setsid sleep 30 & echo $! > {}; echo hi", pidfile.display())));
    let start = std::time::Instant::now();
    exec(&mut e, r#"$h = process_watch(["sh", "-c", $command])"#).await.unwrap();
    // Install cleanup before pumping, including on timeout/assertion failure.
    for _ in 0..100 {
        if pidfile.exists() { break; }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let _cleanup = Cleanup(std::fs::read_to_string(pidfile).unwrap().trim().parse().unwrap());
    tokio::time::timeout(Duration::from_millis(1200), e.run_event_pump()).await.unwrap().unwrap();
    assert!(start.elapsed() < Duration::from_millis(1200));
    assert_eq!(exec(&mut e, "$lines[0].line").await.unwrap(), Value::String("hi".into()));
    assert_eq!(exec(&mut e, "$exits[0].detached_output").await.unwrap(), Value::Bool(true));
    assert_eq!(exec(&mut e, "len($exits)").await.unwrap(), Value::Number(1.0));
}

#[tokio::test(flavor = "current_thread")]
async fn ordered_lines_then_exactly_one_terminal_in_plain_script() {
    let mut e = watcher().await;
    exec(
        &mut e,
        r#"$h = process_watch(["seq", "1", "20"], {tag: "ordered"})"#,
    )
    .await
    .unwrap();
    pump(&mut e).await;
    let Value::List(ref lines) = e.get_global("lines").unwrap() else {
        panic!()
    };
    assert_eq!(lines.len(), 20);
    assert_eq!(
        e.get_global("last_line_count").unwrap(),
        Value::Number(20.0)
    );
    for (n, line) in lines.iter().enumerate() {
        let Value::Map(m) = line else { panic!() };
        assert_eq!(m["line"], Value::String((n + 1).to_string()));
        assert_eq!(m["tag"], Value::String("ordered".into()));
        assert_eq!(m["stream"], Value::String("stdout".into()));
        assert_eq!(m["truncated"], Value::Bool(false));
    }
    let Value::List(ref exits) = e.get_global("exits").unwrap() else {
        panic!()
    };
    assert_eq!(exits.len(), 1);
    let Value::Map(m) = &exits[0] else { panic!() };
    assert_eq!(m["exit_code"], Value::Number(0.0));
    pump(&mut e).await;
    assert_eq!(
        exec(&mut e, "len($exits)").await.unwrap(),
        Value::Number(1.0)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn truncates_and_flushes_unterminated_line() {
    let mut e = watcher().await;
    exec(
        &mut e,
        r#"$h = process_watch(["printf", "abcdef\nxyz"], {max_line: 3})"#,
    )
    .await
    .unwrap();
    pump(&mut e).await;
    let Value::List(ref lines) = e.get_global("lines").unwrap() else {
        panic!()
    };
    assert_eq!(lines.len(), 2);
    for (line, (text, truncated)) in lines.iter().zip([("abc", true), ("xyz", false)]) {
        let Value::Map(m) = line else { panic!() };
        assert_eq!(m["line"], Value::String(text.into()));
        assert_eq!(m["truncated"], Value::Bool(truncated));
    }
    e.set_global("long_line", Value::String("x".repeat(70000) + "\nnext\n"));
    exec(
        &mut e,
        r#"$lines = []; $h = process_watch(["printf", $long_line])"#,
    )
    .await
    .unwrap();
    pump(&mut e).await;
    let Value::List(ref lines) = e.get_global("lines").unwrap() else {
        panic!()
    };
    assert_eq!(lines.len(), 2);
    let Value::Map(line) = &lines[0] else {
        panic!()
    };
    assert_eq!(line["line"].to_mix_string().len(), 65536);
    assert_eq!(line["truncated"], Value::Bool(true));
    let Value::Map(line) = &lines[1] else {
        panic!()
    };
    assert_eq!(line["line"], Value::String("next".into()));
}

#[tokio::test(flavor = "current_thread")]
async fn stderr_modes_and_shared_environment_validation() {
    for mode in ["null", "lines", "inherit"] {
        let mut e = watcher().await;
        e.set_global("mode", Value::String(mode.into()));
        // env's invalid option produces stderr without requiring any shell.
        exec(&mut e, r#"$h = process_watch(["/usr/bin/env", "--mix-no-such-option"], {stderr: $mode, clear_env: true, env: {LC_ALL: "C"}})"#).await.unwrap();
        pump(&mut e).await;
        let Value::List(ref lines) = e.get_global("lines").unwrap() else {
            panic!()
        };
        assert_eq!(!lines.is_empty(), mode == "lines");
        for line in lines.iter() {
            let Value::Map(m) = line else { panic!() };
            assert_eq!(m["stream"], Value::String("stderr".into()));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cwd_and_environment_reach_direct_child() {
    let mut e = watcher().await;
    exec(&mut e, r#"$h = process_watch(["/usr/bin/printenv", "MIX_WATCH_TEST"], {clear_env: true, env: {MIX_WATCH_TEST: 42, REMOVE_ME: nil}})"#).await.unwrap();
    pump(&mut e).await;
    let Value::List(ref lines) = e.get_global("lines").unwrap() else {
        panic!()
    };
    let Value::Map(line) = &lines[0] else {
        panic!()
    };
    assert_eq!(line["line"], Value::String("42".into()));
    exec(
        &mut e,
        r#"$lines = []; $h = process_watch(["/usr/bin/pwd"], {cwd: "/tmp"})"#,
    )
    .await
    .unwrap();
    pump(&mut e).await;
    let Value::List(ref lines) = e.get_global("lines").unwrap() else {
        panic!()
    };
    let Value::Map(line) = &lines[0] else {
        panic!()
    };
    assert_eq!(line["line"], Value::String("/tmp".into()));
}

#[tokio::test(flavor = "current_thread")]
async fn unwatch_and_generation_shutdown_reap_sleep() {
    for unwatch in [true, false] {
        let mut e = watcher().await;
        // Read the owned PID through /proc rather than exposing one in the API.
        exec(
            &mut e,
            r#"$h = process_watch(["sleep", "1000"])
            $h2 = process_watch(["sleep", "1000"])
        "#,
        )
        .await
        .unwrap();
        let children = std::fs::read_to_string(format!("/proc/self/task/{}/children", unsafe {
            libc::syscall(libc::SYS_gettid)
        }))
        .unwrap();
        let pids: Vec<i32> = children
            .split_whitespace()
            .map(|s| s.parse().unwrap())
            .collect();
        assert!(pids.len() >= 2);
        if unwatch {
            assert_eq!(
                exec(&mut e, "process_unwatch($h)").await.unwrap(),
                Value::Bool(true)
            );
            assert_eq!(
                exec(&mut e, "process_unwatch($h)").await.unwrap(),
                Value::Bool(false)
            );
        }
        e.close_native_events();
        for pid in pids {
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "leftover {pid}");
            let mut status = 0;
            assert_eq!(
                unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
                -1
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn reload_candidate_and_bad_options_never_start_children() {
    let mut e = Evaluator::new();
    e.set_reload_candidate(true);
    let error = exec(&mut e, r#"process_watch(["sleep", "1000"])"#)
        .await
        .unwrap_err();
    assert!(
        matches!(error, mix::MixError::Structured(info) if info.code == "PROC_WATCH_RELOAD_CANDIDATE")
    );
    e.set_reload_candidate(false);
    for source in [
        r#"process_watch([])"#,
        r#"process_watch(["sleep", 1])"#,
        r#"process_watch(["true"], {max_queue: 0})"#,
        r#"process_watch(["true"], {max_line: 1.5})"#,
        r#"process_watch(["true"], {stderr: "capture"})"#,
        r#"process_watch(["true"], {env: {"bad=name": "x"}})"#,
        r#"process_watch(["true"], {detach: true})"#,
    ] {
        assert!(exec(&mut e, source).await.is_err(), "{source}");
    }
}

struct Bus {
    rx: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<mix::evaluator::IncomingEvent>>,
    ready: tokio::sync::Notify,
}
impl mix::evaluator::BusHandler for Bus {
    fn send<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a Value,
    ) -> mix::evaluator::BusFuture<'a, MixResult<(i32, Value)>> {
        Box::pin(async { Ok((0, Value::Nil)) })
    }
    fn emit<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a Value,
    ) -> mix::evaluator::BusFuture<'a, MixResult<()>> {
        Box::pin(async {
            self.ready.notify_one();
            Ok(())
        })
    }
    fn port_exists<'a>(&'a self, _: &'a str) -> mix::evaluator::BusFuture<'a, MixResult<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn next_incoming<'a>(
        &'a self,
    ) -> mix::evaluator::BusFuture<'a, Option<mix::evaluator::IncomingEvent>> {
        Box::pin(async {
            self.ready.notified().await;
            self.rx.lock().await.recv().await
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn bus_ping_is_dispatched_while_a_child_remains_alive() {
    let mut e = watcher().await;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    e.set_bus_handler(std::rc::Rc::new(Bus {
        rx: tokio::sync::Mutex::new(rx),
        ready: tokio::sync::Notify::new(),
    }));
    // The inbound ping becomes ready only after a line handler emits.
    exec(
        &mut e,
        r#"
        on proc.line
            $lines = concat($lines, [$event.args])
            emit probe ready
        end
        on ping
            $pong = true
            quit()
        end
        $h = process_watch(["yes", "streaming"])
    "#,
    )
    .await
    .unwrap();
    tx.send(mix::evaluator::IncomingEvent {
        generation: 0,
        command: "ping".into(),
        body: "{}".into(),
        headers: Default::default(),
    })
    .unwrap();
    pump(&mut e).await;
    assert_eq!(e.get_global("pong"), Some(Value::Bool(true)));
    assert_ne!(
        exec(&mut e, "len($lines)").await.unwrap(),
        Value::Number(0.0)
    );
    assert_eq!(
        exec(&mut e, "len($exits)").await.unwrap(),
        Value::Number(0.0)
    );
    e.close_native_events();
}
