// SPDX-License-Identifier: MIT OR Apache-2.0
//! Ctrl-C ends every infinite blocking tcp_accept, even when SIGINT lands
//! on another thread (it cannot interrupt the waiters' poll(2)) and even
//! with several waiters, where one may drain the wake byte another
//! needed. Its own test binary, because interrupt::init is process-wide
//! and leaves the flag set.
#![cfg(target_os = "linux")]
use mix::{MixResult, evaluator::Evaluator, lexer::Lexer, parser::Parser, value::Value};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

async fn exec(eval: &mut Evaluator, source: &str) -> MixResult<Value> {
    let tokens = Lexer::new(source).tokenize()?;
    let stmts = Parser::new(tokens, source).parse_program()?;
    eval.execute(&stmts).await
}

/// One evaluator on its own thread, blocked in tcp_accept with no timeout.
/// Reports whether the accept returned nil.
fn spawn_waiter(done: std::sync::mpsc::Sender<(bool, String)>) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let outcome = rt.block_on(async {
            let mut e = Evaluator::new();
            exec(
                &mut e,
                r#"
$l = tcp_listen("127.0.0.1", 0)
$r = tcp_accept($l, {timeout: 0})
"#,
            )
            .await
            .map(|_| e.get_global("r"))
        });
        let returned_nil = matches!(outcome, Ok(Some(Value::Nil)));
        done.send((returned_nil, format!("{outcome:?}"))).unwrap();
    });
}

#[test]
fn ctrl_c_on_another_thread_ends_every_infinite_accept() {
    assert!(mix::interrupt::init(Arc::new(AtomicBool::new(false))));
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    spawn_waiter(done_tx.clone());
    spawn_waiter(done_tx);
    // Synchronise on both accepts entering their wait loop, not on
    // elapsed time: the flag is unset until then, so each has really
    // started waiting.
    let started = Instant::now();
    while mix::builtins::tcp_accept_waits_entered() < 2 {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "both accepts reach their wait"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Signal THIS thread: neither waiter's poll is interrupted, so only
    // the wake socket or the bounded recheck can end the waits.
    signal_hook::low_level::raise(signal_hook::consts::SIGINT).unwrap();
    for _ in 0..2 {
        let (returned_nil, outcome) = done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("tcp_accept returned after Ctrl-C");
        assert!(returned_nil, "an interrupted accept returns nil: {outcome}");
    }
}
