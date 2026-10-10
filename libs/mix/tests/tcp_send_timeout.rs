// SPDX-License-Identifier: MIT OR Apache-2.0
//! tcp_send's `{timeout}` bounds the WHOLE send through the real
//! evaluator, on loopback: a peer that never reads and a peer that drains
//! a few bytes at a time both fail at about the timeout (not at timeout ×
//! partial writes), the handle is retired with TCP_SEND_TIMEOUT, a fast
//! reader still succeeds, and a tcp_on handle honours the same option.
#![cfg(target_os = "linux")]
use mix::{MixResult, evaluator::Evaluator, lexer::Lexer, parser::Parser, value::Value};
use std::io::Read;
use std::time::{Duration, Instant};

/// Larger than loopback's socket buffers, even autotuned.
const BIG: f64 = 64.0 * 1024.0 * 1024.0;
/// A subscribed send is capped at one 16 MiB frame; still larger than the
/// loopback buffers.
const BIG_FRAME: f64 = 16.0 * 1024.0 * 1024.0;

async fn exec(eval: &mut Evaluator, source: &str) -> MixResult<Value> {
    let tokens = Lexer::new(source).tokenize()?;
    let stmts = Parser::new(tokens, source).parse_program()?;
    eval.execute(&stmts).await
}

fn global(e: &Evaluator, name: &str) -> String {
    e.get_global(name)
        .unwrap_or_else(|| panic!("${name} unset"))
        .to_mix_string()
}

/// Send BIG bytes with a 0.5 s timeout to a peer served by `peer`, and
/// check it fails with TCP_SEND_TIMEOUT at about 0.5 s and retires the
/// handle. `subscribe` sends on a tcp_on handle instead.
async fn times_out(peer: impl FnOnce(std::net::TcpStream) + Send + 'static, subscribe: bool) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        peer(stream);
    });
    let mut e = Evaluator::new();
    e.set_global("port", Value::Number(port as f64));
    let big = if subscribe { BIG_FRAME } else { BIG };
    e.set_global("big", Value::Number(big));
    e.set_global("subscribe", Value::Bool(subscribe));
    let started = Instant::now();
    exec(
        &mut e,
        r#"
$h = tcp_connect("127.0.0.1", $port, {timeout: 5})
if $subscribe then
  tcp_on($h, "test.frame")
end
$payload = repeat("x", $big)
$t0 = monotonic()
try
  tcp_send($h, $payload, {timeout: 0.5})
  $code = "no error"
catch $msg, $err
  $code = $err.code
  $written = $err.details.written
end
$elapsed = monotonic() - $t0
"#,
    )
    .await
    .unwrap();
    assert_eq!(global(&e, "code"), "TCP_SEND_TIMEOUT");
    let elapsed = e.get_global("elapsed").and_then(|v| v.to_number()).unwrap();
    assert!(
        (0.4..2.0).contains(&elapsed),
        "the whole send was bounded by 0.5 s, took {elapsed} s"
    );
    if !subscribe {
        let written = e.get_global("written").and_then(|v| v.to_number()).unwrap();
        assert!(written < big, "a partial write reports {written} bytes");
        // Retired: a partly written stream is unusable.
        exec(&mut e, r#"$closed = tcp_close($h)"#).await.unwrap();
        assert_eq!(global(&e, "closed"), "false");
    }
    assert!(started.elapsed() < Duration::from_secs(30));
    drop(e);
    server.join().unwrap();
}

/// (a) A peer that accepts and never reads.
#[tokio::test(flavor = "current_thread")]
async fn a_peer_that_never_reads_times_out_at_the_deadline() {
    times_out(
        |stream| {
            std::thread::sleep(Duration::from_secs(3));
            drop(stream);
        },
        false,
    )
    .await;
}

/// (b) A slow reader draining a few bytes at a time keeps every partial
/// write making progress; the send must still stop at the deadline, not
/// at timeout × the number of partial writes.
#[tokio::test(flavor = "current_thread")]
async fn a_slow_reader_cannot_stretch_the_send_past_the_deadline() {
    times_out(
        |mut stream| {
            stream
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let until = Instant::now() + Duration::from_secs(3);
            let mut buf = [0u8; 64];
            while Instant::now() < until {
                if matches!(stream.read(&mut buf), Ok(0)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        },
        false,
    )
    .await;
}

/// The same option on a tcp_on handle: the receipt deadline.
#[tokio::test(flavor = "current_thread")]
async fn a_subscribed_handle_honours_the_send_timeout() {
    times_out(
        |stream| {
            std::thread::sleep(Duration::from_secs(3));
            drop(stream);
        },
        true,
    )
    .await;
}

/// (c) A fast reader: the timed send succeeds and the handle stays usable.
#[tokio::test(flavor = "current_thread")]
async fn a_fast_reader_succeeds_with_a_timeout_set() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut total = 0usize;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => total += n,
            }
        }
        total
    });
    let mut e = Evaluator::new();
    e.set_global("port", Value::Number(port as f64));
    exec(
        &mut e,
        r#"
$h = tcp_connect("127.0.0.1", $port, {timeout: 5})
$sent = tcp_send($h, repeat("y", 8388608), {timeout: 10})
$more = tcp_send($h, "tail")
$closed = tcp_close($h)
"#,
    )
    .await
    .unwrap();
    assert_eq!(
        e.get_global("sent").and_then(|v| v.to_number()),
        Some(8388608.0)
    );
    assert_eq!(e.get_global("more").and_then(|v| v.to_number()), Some(4.0));
    assert_eq!(global(&e, "closed"), "true");
    assert_eq!(server.join().unwrap(), 8388608 + 4);
}
