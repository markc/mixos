// SPDX-License-Identifier: MIT OR Apache-2.0
//! Server-side TCP through the real evaluator, on loopback: tcp_listen,
//! tcp_local_addr, tcp_accept (blocking, timeout and Class C), tcp_close
//! on a listener, the structured error codes, generation ownership, and
//! tcp_on on a listener delivering `accepted` events that the handler
//! then subscribes for lines.
#![cfg(target_os = "linux")]
use mix::{
    MixResult,
    evaluator::{BusFuture, BusHandler, Evaluator, IncomingEvent, ReservedOutcome, ServeRuntime},
    lexer::Lexer,
    parser::Parser,
    value::Value,
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    rc::Rc,
    time::{Duration, Instant},
};

struct Runtime;
impl ServeRuntime for Runtime {
    fn handle_reserved(
        &self,
        command: &str,
        _: Option<&str>,
        _: &str,
        _: &[(&str, Option<&str>)],
        _: bool,
    ) -> Option<ReservedOutcome> {
        (command == "RELOAD").then(|| ReservedOutcome {
            rc: 0,
            body: "{}".into(),
            quit: false,
            reload: true,
        })
    }
}

struct Bus {
    rx: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<IncomingEvent>>,
}
impl BusHandler for Bus {
    fn send<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: &'a Value,
    ) -> BusFuture<'a, MixResult<(i32, Value)>> {
        Box::pin(async { Ok((0, Value::Nil)) })
    }
    fn emit<'a>(&'a self, _: &'a str, _: &'a str, _: &'a Value) -> BusFuture<'a, MixResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn port_exists<'a>(&'a self, _: &'a str) -> BusFuture<'a, MixResult<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn next_incoming<'a>(&'a self) -> BusFuture<'a, Option<IncomingEvent>> {
        Box::pin(async { self.rx.lock().await.recv().await })
    }
}

fn event(command: &str) -> IncomingEvent {
    IncomingEvent {
        generation: 0,
        command: command.into(),
        body: "{}".into(),
        headers: BTreeMap::new(),
    }
}

async fn exec(eval: &mut Evaluator, source: &str) -> MixResult<Value> {
    let tokens = Lexer::new(source).tokenize()?;
    let stmts = Parser::new(tokens, source).parse_program()?;
    eval.execute(&stmts).await
}

/// An evaluator wired like a `--serve` citizen: a serve runtime and a Bus
/// whose incoming events the test drives.
fn served() -> (Evaluator, tokio::sync::mpsc::UnboundedSender<IncomingEvent>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut e = Evaluator::new();
    e.set_serve_runtime(Rc::new(Runtime));
    e.set_bus_handler(Rc::new(Bus {
        rx: tokio::sync::Mutex::new(rx),
    }));
    (e, tx)
}

fn global_str(e: &Evaluator, name: &str) -> String {
    e.get_global(name)
        .unwrap_or_else(|| panic!("${name} unset"))
        .to_mix_string()
}

fn global_port(e: &Evaluator, name: &str) -> u16 {
    let n = e
        .get_global(name)
        .and_then(|v| v.to_number())
        .unwrap_or_else(|| panic!("${name} is not a number"));
    assert!(n > 0.0 && n <= 65535.0, "${name} = {n}");
    n as u16
}

#[tokio::test(flavor = "current_thread")]
async fn listen_on_port_zero_reports_its_address() {
    let mut e = Evaluator::new();
    exec(
        &mut e,
        r#"
$l = tcp_listen("127.0.0.1", 0, {backlog: 8})
$a = tcp_local_addr($l)
$host = $a.host
$port = $a.port
"#,
    )
    .await
    .unwrap();
    assert_eq!(global_str(&e, "host"), "127.0.0.1");
    let port = global_port(&e, "port");
    // The kernel completes the handshake from the backlog: the port is
    // really listening.
    std::net::TcpStream::connect(("127.0.0.1", port)).expect("listening");
    e.close_native_events();
}

#[tokio::test(flavor = "current_thread")]
async fn accept_and_line_round_trip_in_one_script() {
    let mut e = Evaluator::new();
    exec(
        &mut e,
        r#"
$l = tcp_listen("127.0.0.1", 0)
$a = tcp_local_addr($l)
$c = tcp_connect("127.0.0.1", $a.port, {timeout: 5})
$s = tcp_accept($l, {timeout: 5})
tcp_send($c, "{\"n\":1}\r\n")
$req = tcp_recv_line($s, {timeout: 5})
tcp_send($s, json_encode({ok: true, echo: json_parse($req)}) .. "\n")
$reply = json_parse(tcp_recv_line($c, {timeout: 5}))
$n = $reply.echo.n
$ok = $reply.ok
$client_port = tcp_local_addr($c).port
$same_port = tcp_local_addr($s).port == $a.port
tcp_close($c)
tcp_close($s)
"#,
    )
    .await
    .unwrap();
    assert_eq!(global_str(&e, "req"), r#"{"n":1}"#);
    assert_eq!(e.get_global("n").unwrap().to_number(), Some(1.0));
    assert_eq!(global_str(&e, "ok"), "true");
    // A connected handle reports its own end: the client its ephemeral
    // port, the accepted handle the listener's port.
    global_port(&e, "client_port");
    assert_eq!(global_str(&e, "same_port"), "true");
    e.close_native_events();
}

/// Data already buffered is served before a timeout is judged: two lines
/// arriving in one write come back one per call even with a timeout so
/// short it has passed before the first look at the socket.
#[tokio::test(flavor = "current_thread")]
async fn buffered_lines_are_served_before_a_tiny_timeout_expires() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"one\ntwo\n").unwrap();
        std::thread::sleep(Duration::from_millis(1500));
    });
    let mut e = Evaluator::new();
    e.set_global("port", Value::Number(port as f64));
    exec(
        &mut e,
        r#"$h = tcp_connect("127.0.0.1", $port, {timeout: 5})"#,
    )
    .await
    .unwrap();
    // Let both lines land in the socket before the first receive.
    std::thread::sleep(Duration::from_millis(200));
    exec(
        &mut e,
        r#"
$first = tcp_recv_line($h, {timeout: 0.000000001})
$second = tcp_recv_line($h, {timeout: 0.000000001})
$third = tcp_recv_line($h, {timeout: 0.000000001})
$none = $third == nil
tcp_close($h)
"#,
    )
    .await
    .unwrap();
    assert_eq!(global_str(&e, "first"), "one");
    assert_eq!(global_str(&e, "second"), "two");
    assert_eq!(global_str(&e, "none"), "true");
    server.join().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn accept_timeout_returns_nil_and_the_listener_stays_usable() {
    let mut e = Evaluator::new();
    let started = Instant::now();
    exec(
        &mut e,
        r#"
$l = tcp_listen("127.0.0.1", 0)
$first = tcp_accept($l, {timeout: 0.2})
$was_nil = $first == nil
$a = tcp_local_addr($l)
$c = tcp_connect("127.0.0.1", $a.port, {timeout: 5})
$second = tcp_accept($l, {timeout: 5})
"#,
    )
    .await
    .unwrap();
    assert_eq!(global_str(&e, "was_nil"), "true");
    assert!(matches!(e.get_global("second"), Some(Value::Number(_))));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a 0.2 s accept timeout took {:?}",
        started.elapsed()
    );
    e.close_native_events();
}

#[tokio::test(flavor = "current_thread")]
async fn errors_carry_stable_codes() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let held_port = held.local_addr().unwrap().port();
    let mut e = Evaluator::new();
    e.set_global("held", Value::Number(held_port as f64));
    exec(
        &mut e,
        r#"
fn code_of($f)
  try
    $f()
  catch $msg, $err
    return $err.code
  end
  return "no error"
end
$in_use = code_of(fn() tcp_listen("127.0.0.1", $held) end)
$bad_port = code_of(fn() tcp_listen("127.0.0.1", 70000) end)
$bad_host = code_of(fn() tcp_listen("", 0) end)
$bad_opt = code_of(fn() tcp_listen("127.0.0.1", 0, {backlog: 0}) end)
$unknown_opt = code_of(fn() tcp_listen("127.0.0.1", 0, {reuse: true}) end)
$l = tcp_listen("127.0.0.1", 0)
$a = tcp_local_addr($l)
$c = tcp_connect("127.0.0.1", $a.port, {timeout: 5})
$recv_listener = code_of(fn() tcp_recv($l, {timeout: 0.1}) end)
$accept_conn = code_of(fn() tcp_accept($c, {timeout: 0.1}) end)
$accept_opt = code_of(fn() tcp_accept($l, {max: 5}) end)
$unknown = code_of(fn() tcp_local_addr(999999999) end)
"#,
    )
    .await
    .unwrap();
    for (name, code) in [
        ("in_use", "TCP_LISTEN_ADDR_IN_USE"),
        ("bad_port", "TCP_LISTEN_ARGUMENT"),
        ("bad_host", "TCP_LISTEN_ARGUMENT"),
        ("bad_opt", "TCP_LISTEN_ARGUMENT"),
        ("unknown_opt", "TCP_LISTEN_ARGUMENT"),
        ("recv_listener", "TCP_LISTENER"),
        ("accept_conn", "TCP_ACCEPT_HANDLE"),
        ("accept_opt", "TCP_ACCEPT_ARGUMENT"),
        ("unknown", "TCP_HANDLE"),
    ] {
        assert_eq!(global_str(&e, name), code, "${name}");
    }
    drop(held);
    e.close_native_events();
}

#[tokio::test(flavor = "current_thread")]
async fn tcp_close_on_a_listener_stops_listening() {
    let mut e = Evaluator::new();
    exec(
        &mut e,
        r#"
$l = tcp_listen("127.0.0.1", 0)
$a = tcp_local_addr($l)
$port = $a.port
$first = tcp_close($l)
$second = tcp_close($l)
try
  tcp_accept($l, {timeout: 0.1})
  $after = "no error"
catch $msg, $err
  $after = $err.code
end
"#,
    )
    .await
    .unwrap();
    assert_eq!(global_str(&e, "first"), "true");
    assert_eq!(global_str(&e, "second"), "false");
    assert_eq!(global_str(&e, "after"), "TCP_ACCEPT_HANDLE");
    let port = global_port(&e, "port");
    let err = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
    e.close_native_events();
}

/// Retiring the generation closes its listener and the connection it
/// accepted; a tcp_connect handle is not the generation's.
#[tokio::test(flavor = "current_thread")]
async fn retiring_the_generation_closes_listeners_and_accepted_connections() {
    let mut e = Evaluator::new();
    exec(
        &mut e,
        r#"
$l = tcp_listen("127.0.0.1", 0)
$a = tcp_local_addr($l)
$port = $a.port
"#,
    )
    .await
    .unwrap();
    let port = global_port(&e, "port");
    let mut client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    exec(&mut e, r#"$s = tcp_accept($l, {timeout: 5})"#)
        .await
        .unwrap();
    assert!(matches!(e.get_global("s"), Some(Value::Number(_))));
    e.close_native_events();
    // The accepted connection was closed: the client reads EOF.
    let mut buf = [0u8; 8];
    assert_eq!(client.read(&mut buf).unwrap(), 0, "accepted handle closed");
    // The listener was closed: the port refuses.
    let err = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
    // Both handles are gone from the registries.
    exec(
        &mut e,
        r#"
$s_closed = tcp_close($s)
$l_closed = tcp_close($l)
"#,
    )
    .await
    .unwrap();
    assert_eq!(global_str(&e, "s_closed"), "false");
    assert_eq!(global_str(&e, "l_closed"), "false");
}

/// The event path: tcp_on on a listener delivers one `accepted` event per
/// connection; the handler subscribes the accepted handle for lines, and
/// the line arrives as its own event. A subscribed listener refuses a
/// numeric tcp_accept.
#[tokio::test(flavor = "current_thread")]
async fn subscribed_listener_delivers_accepted_then_line_events() {
    let (mut e, _tx) = served();
    exec(
        &mut e,
        r#"
$l = tcp_listen("127.0.0.1", 0)
$a = tcp_local_addr($l)
$port = $a.port
$src = tcp_on($l, "srv.conn")
try
  tcp_accept($l, {timeout: 0.1})
  $refused = "no error"
catch $msg, $err
  $refused = $err.code
end
$sub_addr = tcp_local_addr($l).port
$peer = nil
$line = nil
on srv.conn
  if $event.args.accepted != nil then
    $peer = $event.args.accepted.peer.host
    tcp_on($event.args.accepted.handle, "srv.line", {frame: "line"})
  end
end
on srv.line
  if $event.args.frame != nil then
    $line = $event.args.frame.data
    quit()
  end
end
"#,
    )
    .await
    .unwrap();
    assert!(global_str(&e, "src").starts_with("tcp:"));
    assert_eq!(global_str(&e, "refused"), "SOCKET_SUBSCRIBED");
    let port = global_port(&e, "port");
    assert_eq!(global_port(&e, "sub_addr"), port);
    let client = std::thread::spawn(move || {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"hello\r\n").unwrap();
        // Hold the connection until the handler has read the line.
        std::thread::sleep(Duration::from_millis(500));
    });
    tokio::time::timeout(Duration::from_secs(120), e.run_event_pump())
        .await
        .expect("pump deadline")
        .unwrap();
    assert_eq!(global_str(&e, "line"), "hello");
    assert_eq!(global_str(&e, "peer"), "127.0.0.1");
    client.join().unwrap();
    e.close_native_events();
    let err = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
}

/// A Class C handler parked in a numeric tcp_accept must not hold the
/// pump: the concurrent handler that makes the connection has to run
/// while the accept waits, or the accept would time out with nil.
#[tokio::test(flavor = "current_thread")]
async fn class_c_accept_yields_to_a_concurrent_handler() {
    let (mut e, tx) = served();
    exec(
        &mut e,
        r#"
$l = tcp_listen("127.0.0.1", 0)
$a = tcp_local_addr($l)
$port = $a.port
$got = nil
$c = nil
on cmd1 async
  $got = tcp_accept($l, {timeout: 10})
  quit()
end
on cmd2
  $c = tcp_connect("127.0.0.1", $port, {timeout: 5})
end
"#,
    )
    .await
    .unwrap();
    let started = Instant::now();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let driver = tokio::task::spawn_local({
                let tx = tx.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    tx.send(event("cmd1")).unwrap();
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    tx.send(event("cmd2")).unwrap();
                }
            });
            tokio::time::timeout(Duration::from_secs(120), e.run_event_pump())
                .await
                .expect("pump deadline")
                .unwrap();
            driver.await.unwrap();
            assert!(
                matches!(e.get_global("got"), Some(Value::Number(_))),
                "the accept returned a handle: {:?}",
                e.get_global("got")
            );
            assert!(
                started.elapsed() < Duration::from_secs(8),
                "the accept waited for its timeout: {:?}",
                started.elapsed()
            );
            e.close_native_events();
        })
        .await;
}
