// SPDX-License-Identifier: MIT OR Apache-2.0
//! A receive whose peer keeps sending things it cannot deliver must still
//! end at its timeout: a WebSocket peer flooding unsolicited Pong frames
//! (control frames, never returned by ws_recv), and a TCP peer trickling
//! bytes with no newline (tcp_recv_line never completes a line). Both make
//! progress without delivering, which must not bypass the deadline.
#![cfg(target_os = "linux")]
use mix::{MixResult, evaluator::Evaluator, lexer::Lexer, parser::Parser, value::Value};
use std::io::Write;
use std::time::{Duration, Instant};

async fn exec(eval: &mut Evaluator, source: &str) -> MixResult<Value> {
    let tokens = Lexer::new(source).tokenize()?;
    let stmts = Parser::new(tokens, source).parse_program()?;
    eval.execute(&stmts).await
}

/// Run `script` (which sets $r and $elapsed) and check it returned nil
/// at about the 0.3 s timeout.
async fn returns_nil_at_the_timeout(port: u16, script: &str) {
    let mut e = Evaluator::new();
    e.set_global("port", Value::Number(port as f64));
    exec(&mut e, script).await.unwrap();
    assert!(
        matches!(e.get_global("r"), Some(Value::Nil)),
        "a timed-out receive returns nil: {:?}",
        e.get_global("r")
    );
    let elapsed = e.get_global("elapsed").and_then(|v| v.to_number()).unwrap();
    assert!(
        (0.25..1.5).contains(&elapsed),
        "the 0.3 s timeout held, took {elapsed} s"
    );
}

/// (a) A WebSocket peer flooding Pong frames and no data.
#[tokio::test(flavor = "current_thread")]
async fn a_pong_flood_cannot_hold_ws_recv_past_its_timeout() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if ws
                .send(tungstenite::Message::Pong(b"flood".to_vec().into()))
                .is_err()
            {
                break;
            }
        }
    });
    returns_nil_at_the_timeout(
        port,
        r#"
$h = ws_connect("ws://127.0.0.1:" .. $port .. "/", {timeout: 5})
$t0 = monotonic()
$r = ws_recv($h, 0.3)
$elapsed = monotonic() - $t0
ws_close($h)
"#,
    )
    .await;
    server.join().unwrap();
}

/// (b) A TCP peer trickling bytes with no newline: tcp_recv_line returns
/// nil at the timeout, and the partial line stays buffered.
#[tokio::test(flavor = "current_thread")]
async fn a_newline_free_trickle_cannot_hold_tcp_recv_line_past_its_timeout() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if stream.write_all(b"x").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    returns_nil_at_the_timeout(
        port,
        r#"
$h = tcp_connect("127.0.0.1", $port, {timeout: 5})
$t0 = monotonic()
$r = tcp_recv_line($h, {timeout: 0.3})
$elapsed = monotonic() - $t0
tcp_close($h)
"#,
    )
    .await;
    server.join().unwrap();
}
