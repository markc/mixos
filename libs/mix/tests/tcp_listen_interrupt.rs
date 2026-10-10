// SPDX-License-Identifier: MIT OR Apache-2.0
//! Ctrl-C ends an infinite blocking tcp_accept even when SIGINT lands on
//! another thread, where it cannot interrupt the accept's poll(2): the
//! interrupt wake socket in the poll set must wake it. Its own test
//! binary, because interrupt::init is process-wide and leaves the flag
//! set.
#![cfg(target_os = "linux")]
use mix::{MixResult, evaluator::Evaluator, lexer::Lexer, parser::Parser, value::Value};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

async fn exec(eval: &mut Evaluator, source: &str) -> MixResult<Value> {
    let tokens = Lexer::new(source).tokenize()?;
    let stmts = Parser::new(tokens, source).parse_program()?;
    eval.execute(&stmts).await
}

#[test]
fn ctrl_c_on_another_thread_ends_an_infinite_accept() {
    assert!(mix::interrupt::init(Arc::new(AtomicBool::new(false))));
    let (done_tx, done_rx) = std::sync::mpsc::channel();
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
        done_tx
            .send((returned_nil, format!("{outcome:?}")))
            .unwrap();
    });
    // Let the accept reach poll(2), then signal THIS thread: the signal
    // cannot interrupt the other thread's poll, only the wake socket can.
    std::thread::sleep(Duration::from_millis(500));
    signal_hook::low_level::raise(signal_hook::consts::SIGINT).unwrap();
    let (returned_nil, outcome) = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("tcp_accept returned after Ctrl-C");
    assert!(returned_nil, "an interrupted accept returns nil: {outcome}");
}
