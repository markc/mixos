// SPDX-License-Identifier: MIT OR Apache-2.0
//! UDP subscriptions through a real serve-style evaluator.
#![cfg(target_os = "linux")]
use mix::{
    MixResult,
    evaluator::{BusFuture, BusHandler, Evaluator, IncomingEvent, ReservedOutcome, ServeRuntime},
    lexer::Lexer,
    parser::Parser,
    value::Value,
};
use std::{rc::Rc, time::Duration};

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

async fn exec(eval: &mut Evaluator, source: &str) -> MixResult<Value> {
    mix_server::register().expect("server test family");
    let tokens = Lexer::new(source).tokenize()?;
    let stmts = Parser::new(tokens, source).parse_program()?;
    eval.execute(&stmts).await
}

/// An evaluator wired like a `--serve` citizen: a serve runtime and a Bus
/// whose incoming events the test drives.
fn served() -> (Evaluator, tokio::sync::mpsc::UnboundedSender<IncomingEvent>) {
    mix_server::register().expect("server test family");
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut e = Evaluator::new();
    e.set_serve_runtime(Rc::new(Runtime));
    e.set_bus_handler(Rc::new(Bus {
        rx: tokio::sync::Mutex::new(rx),
    }));
    (e, tx)
}

fn global_str(e: &Evaluator, name: &str) -> String {
    mix_server::register().expect("server test family");
    e.get_global(name)
        .unwrap_or_else(|| panic!("${name} unset"))
        .to_mix_string()
}

#[tokio::test(flavor = "current_thread")]
async fn udp_handler_replies_from_listener_and_generation_retires() {
    mix_server::register().expect("server test family");
    let (mut e, _tx) = served();
    exec(
        &mut e,
        r#"
$h = udp_listen("127.0.0.1", 0)
$addr = udp_local_addr($h)
$port = $addr.port
$host = $addr.host
$watch = udp_on($h, "test.packet")
$sub_port = udp_local_addr($h).port
$received = nil
$kind = nil
$wire_hex = nil
on test.packet
  $received = $event.args.datagram.text
  $kind = type($event.args.datagram.bytes)
  $wire_hex = json_parse($event.body).datagram.bytes.hex
  udp_send_via($h, $event.args.datagram.from_host,
    $event.args.datagram.from_port, $event.args.datagram.bytes)
  quit()
end
"#,
    )
    .await
    .unwrap();
    let port = e.get_global("port").unwrap().to_number().unwrap() as u16;
    assert_ne!(port, 0);
    assert_eq!(global_str(&e, "host"), "127.0.0.1");
    assert_eq!(global_str(&e, "sub_port"), port.to_string());
    let client = std::thread::spawn(move || {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        socket.send_to(b"hello", ("127.0.0.1", port)).unwrap();
        let mut data = [0; 16];
        let (n, from) = socket.recv_from(&mut data).unwrap();
        assert_eq!(&data[..n], b"hello");
        assert_eq!(from.port(), port);
    });
    tokio::time::timeout(Duration::from_secs(10), e.run_event_pump())
        .await
        .unwrap()
        .unwrap();
    client.join().unwrap();
    assert_eq!(global_str(&e, "received"), "hello");
    assert_eq!(global_str(&e, "kind"), "bytes");
    assert_eq!(global_str(&e, "wire_hex"), "68656c6c6f");
    e.close_native_events();
    let bound = std::net::UdpSocket::bind(("127.0.0.1", port)).unwrap();
    drop(bound);
    exec(&mut e, "$closed = udp_close($h)").await.unwrap();
    assert_eq!(global_str(&e, "closed"), "false");
    exec(
        &mut e,
        r#"
try
  udp_local_addr($h)
catch $msg, $err
  $closed_addr = $err.code
end
"#,
    )
    .await
    .unwrap();
    assert_eq!(global_str(&e, "closed_addr"), "UDP_HANDLE");
}
