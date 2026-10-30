// SPDX-License-Identifier: MIT OR Apache-2.0
//! process_watch stdin pipes and lossless (`overflow: "block"`) watches:
//! the two-way child pipe a framed protocol such as ACP needs.
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
        $overflows = 0
        on proc.line
            $lines = concat($lines, [$event.args])
        end
        on proc.overflow
            $overflows = $overflows + 1
        end
        on proc.watch_exited
            $exits = concat($exits, [$event.args])
        end
    "#,
    )
    .await
    .unwrap();
    e
}

fn code(error: mix::MixError) -> String {
    match error {
        mix::MixError::Structured(info) => info.code.to_string(),
        other => format!("{other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn written_lines_round_trip_and_close_gives_eof() {
    let mut e = watcher().await;
    exec(
        &mut e,
        r#"
        $h = process_watch(["cat"], {stdin: "pipe", overflow: "block"})
        $a = process_write($h, "{\"jsonrpc\":\"2.0\",\"id\":1}\n")
        $b = process_write($h, string_to_bytes("second\n"))
        $c = process_close_stdin($h)
        $d = process_close_stdin($h)
        $f = process_write($h, "late\n")
    "#,
    )
    .await
    .unwrap();
    for (name, want) in [
        ("a", true),
        ("b", true),
        ("c", true),
        ("d", false),
        ("f", false),
    ] {
        assert_eq!(e.get_global(name), Some(Value::Bool(want)), "${name}");
    }
    // cat exits on EOF, so the pump ends with the terminal record.
    pump(&mut e).await;
    assert_eq!(
        exec(&mut e, "$lines[0].line").await.unwrap(),
        Value::String(r#"{"jsonrpc":"2.0","id":1}"#.into())
    );
    assert_eq!(
        exec(&mut e, "$lines[1].line").await.unwrap(),
        Value::String("second".into())
    );
    assert_eq!(
        exec(&mut e, "$exits[0].exit_code").await.unwrap(),
        Value::Number(0.0)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_watch_without_a_pipe_refuses_writes() {
    let mut e = Evaluator::new();
    exec(&mut e, r#"$h = process_watch(["sleep", "30"])"#)
        .await
        .unwrap();
    for source in [r#"process_write($h, "x")"#, "process_close_stdin($h)"] {
        let error = exec(&mut e, source).await.unwrap_err();
        assert_eq!(code(error), "PROC_STDIN_NOT_PIPED", "{source}");
    }
    assert_eq!(
        exec(&mut e, r#"process_write("process:0", "x")"#)
            .await
            .unwrap(),
        Value::Bool(false)
    );
    e.close_native_events();
}

#[tokio::test(flavor = "current_thread")]
async fn writes_are_all_or_nothing_against_stdin_buffer() {
    let mut e = Evaluator::new();
    // sleep never reads, so accepted bytes stay buffered past the pipe.
    exec(
        &mut e,
        r#"
        $h = process_watch(["sleep", "30"], {stdin: "pipe", stdin_buffer: 4})
        $big = process_write($h, "hello")
        $fits = process_write($h, "hey")
    "#,
    )
    .await
    .unwrap();
    assert_eq!(e.get_global("big"), Some(Value::Bool(false)));
    assert_eq!(e.get_global("fits"), Some(Value::Bool(true)));
    e.close_native_events();
}

#[tokio::test(flavor = "current_thread")]
async fn writes_after_the_child_exits_return_false() {
    let mut e = watcher().await;
    exec(&mut e, r#"$h = process_watch(["true"], {stdin: "pipe"})"#)
        .await
        .unwrap();
    pump(&mut e).await;
    assert_eq!(
        exec(&mut e, r#"process_write($h, "x\n")"#).await.unwrap(),
        Value::Bool(false)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_child_that_closes_its_stdin_refuses_later_writes() {
    let mut e = Evaluator::new();
    // The shell drops its only copy of the read end, then idles. Nothing has
    // been written, so the worker must see the hang-up without POLLOUT.
    exec(
        &mut e,
        r#"$h = process_watch(["sh", "-c", "exec 0<&-; echo closed; sleep 30"], {stdin: "pipe"})"#,
    )
    .await
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        exec(&mut e, "sleep(0.05)").await.unwrap();
        if exec(&mut e, r#"process_write($h, "x\n")"#).await.unwrap() == Value::Bool(false) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "closed stdin never refused a write"
        );
    }
    e.close_native_events();
}

#[tokio::test(flavor = "current_thread")]
async fn block_delivers_every_line_in_order_past_a_tiny_queue() {
    let mut e = watcher().await;
    exec(
        &mut e,
        r#"$h = process_watch(["seq", "1", "5000"], {overflow: "block", max_queue: 4})"#,
    )
    .await
    .unwrap();
    pump(&mut e).await;
    assert_eq!(
        exec(&mut e, "$overflows").await.unwrap(),
        Value::Number(0.0)
    );
    assert_eq!(
        exec(&mut e, "len($lines)").await.unwrap(),
        Value::Number(5000.0)
    );
    assert_eq!(
        exec(&mut e, "$lines[4999].line").await.unwrap(),
        Value::String("5000".into())
    );
    assert_eq!(
        exec(&mut e, "$exits[0].exit_code").await.unwrap(),
        Value::Number(0.0)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn drop_mode_still_reports_overflow() {
    let mut e = watcher().await;
    // Nothing drains until the pump runs, so a 4-line queue must drop.
    exec(
        &mut e,
        r#"$h = process_watch(["seq", "1", "5000"], {max_queue: 4})"#,
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    pump(&mut e).await;
    assert!(matches!(
        exec(&mut e, "$overflows").await.unwrap(),
        Value::Number(n) if n >= 1.0
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn block_ends_the_watch_on_an_overlong_line_instead_of_truncating() {
    let mut e = watcher().await;
    exec(
        &mut e,
        r#"$h = process_watch(["sh", "-c", "echo ok; printf 'aaaaaaaaaaaaaaaaaaaa\n'; sleep 30"], {overflow: "block", max_line: 8})"#,
    )
    .await
    .unwrap();
    pump(&mut e).await;
    assert_eq!(
        exec(&mut e, "len($lines)").await.unwrap(),
        Value::Number(1.0)
    );
    assert_eq!(
        exec(&mut e, "$lines[0].line").await.unwrap(),
        Value::String("ok".into())
    );
    assert_eq!(
        exec(&mut e, "$exits[0].error_code").await.unwrap(),
        Value::String("PROC_LINE_LIMIT".into())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_large_frame_crosses_both_pipes_intact() {
    let mut e = watcher().await;
    // 3 MiB is far past both the pipe buffer and one 4 KiB read.
    exec(
        &mut e,
        r#"
        $frame = repeat("x", 3145728)
        $h = process_watch(["cat"], {stdin: "pipe", overflow: "block", max_line: 4194304})
        $ok = process_write($h, $frame .. "\n")
        process_close_stdin($h)
    "#,
    )
    .await
    .unwrap();
    assert_eq!(e.get_global("ok"), Some(Value::Bool(true)));
    pump(&mut e).await;
    assert_eq!(
        exec(&mut e, "len($lines[0].line)").await.unwrap(),
        Value::Number(3145728.0)
    );
    assert_eq!(
        exec(&mut e, "$lines[0].truncated").await.unwrap(),
        Value::Bool(false)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn bad_stdin_options_never_start_children() {
    let mut e = Evaluator::new();
    for source in [
        r#"process_watch(["true"], {stdin: "inherit"})"#,
        r#"process_watch(["true"], {stdin: true})"#,
        r#"process_watch(["true"], {stdin_buffer: 16})"#,
        r#"process_watch(["true"], {stdin: "pipe", stdin_buffer: 0})"#,
        r#"process_watch(["true"], {stdin: "pipe", stdin_buffer: 67108865})"#,
        r#"process_watch(["true"], {overflow: "wait"})"#,
    ] {
        assert!(exec(&mut e, source).await.is_err(), "{source}");
    }
}
