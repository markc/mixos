// SPDX-License-Identifier: MIT OR Apache-2.0
//! unix_connect through the real evaluator: a unix-domain stream socket in
//! tcp_connect's handle space, NUL-terminated frames (Varlink and hostd
//! framing) through tcp_recv_line on both its paths (the Class C pull and
//! the blocking builtin) and through tcp_on line events, LF lines by
//! default, and the structured refusals.
#![cfg(target_os = "linux")]
use mix::{
    MixResult,
    evaluator::{BusFuture, BusHandler, Evaluator, IncomingEvent, ReservedOutcome, ServeRuntime},
    lexer::Lexer,
    parser::Parser,
    value::Value,
};
use std::{
    io::{Read, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

struct Runtime;
impl ServeRuntime for Runtime {
    fn handle_reserved(
        &self,
        _: &str,
        _: Option<&str>,
        _: &str,
        _: &[(&str, Option<&str>)],
        _: bool,
    ) -> Option<ReservedOutcome> {
        None
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
    let tokens = Lexer::new(source).tokenize()?;
    let stmts = Parser::new(tokens, source).parse_program()?;
    eval.execute(&stmts).await
}

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

/// A fresh socket path under the system temp dir, unique per test.
fn sock_path(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let p = std::env::temp_dir().join(format!(
        "mix-unix-{}-{}-{tag}.sock",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&p);
    p
}

/// Read NUL-terminated frames from `s` until `n` have arrived.
fn read_frames(s: &mut impl Read, n: usize) -> Vec<String> {
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut tmp = [0u8; 4096];
    while out.len() < n {
        let k = s.read(&mut tmp).unwrap();
        assert!(k > 0, "peer closed early");
        buf.extend_from_slice(&tmp[..k]);
        while let Some(i) = buf.iter().position(|&b| b == 0) {
            let frame: Vec<u8> = buf.drain(..=i).collect();
            out.push(String::from_utf8(frame[..frame.len() - 1].to_vec()).unwrap());
        }
    }
    out
}

/// hostd's shape: two NUL-framed requests on one connection, the two
/// replies arriving in ONE write (so the second is served from the
/// read-ahead), a CR before a NUL kept (only LF framing strips CR).
#[tokio::test(flavor = "current_thread")]
async fn nul_frames_round_trip_on_one_connection() {
    let path = sock_path("rt");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let reqs = read_frames(&mut s, 2);
        let mut reply = Vec::new();
        for r in &reqs {
            reply.extend_from_slice(format!(r#"{{"rc":0,"body":{r}}}"#).as_bytes());
            reply.push(0);
        }
        reply.extend_from_slice(b"cr\r\0");
        s.write_all(&reply).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        reqs
    });
    let mut e = Evaluator::new();
    e.set_global("path", Value::String(path.to_string_lossy().into_owned()));
    exec(
        &mut e,
        r#"
$h = unix_connect($path, {frame: "nul", timeout: 5})
tcp_send($h, json_encode({verb: "host.info", body: {}}) .. chr(0))
tcp_send($h, json_encode({verb: "net.state", body: {}}) .. chr(0))
$one = json_parse(tcp_recv_line($h, {timeout: 5}))
$two = json_parse(tcp_recv_line($h, {timeout: 5}))
$cr = tcp_recv_line($h, {timeout: 5})
$verb1 = $one.body.verb
$verb2 = $two.body.verb
$rc = $one.rc
$cr_kept = $cr == "cr\r"
$no_addr = "none"
try
  tcp_local_addr($h)
catch $m, $err
  $no_addr = $err.code
end
tcp_close($h)
"#,
    )
    .await
    .unwrap();
    let reqs = server.join().unwrap();
    assert!(reqs[0].contains(r#""verb":"host.info""#), "{}", reqs[0]);
    assert!(reqs[1].contains(r#""verb":"net.state""#), "{}", reqs[1]);
    assert_eq!(global_str(&e, "verb1"), "host.info");
    assert_eq!(global_str(&e, "verb2"), "net.state");
    assert_eq!(global_str(&e, "rc"), "0");
    assert_eq!(global_str(&e, "cr_kept"), "true");
    assert_eq!(global_str(&e, "no_addr"), "TCP_LOCAL_ADDR");
    let _ = std::fs::remove_file(&path);
}

/// Without `frame`, a unix handle reads LF lines, CR stripped, as
/// tcp_connect does.
#[tokio::test(flavor = "current_thread")]
async fn default_frame_is_lf_lines() {
    let path = sock_path("lf");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.write_all(b"one\r\ntwo\0still two\n").unwrap();
        std::thread::sleep(Duration::from_millis(500));
    });
    let mut e = Evaluator::new();
    e.set_global("path", Value::String(path.to_string_lossy().into_owned()));
    exec(
        &mut e,
        r#"
$h = unix_connect($path)
$a = tcp_recv_line($h, {timeout: 5})
$b = tcp_recv_line($h, {timeout: 5})
tcp_close($h)
"#,
    )
    .await
    .unwrap();
    server.join().unwrap();
    assert_eq!(global_str(&e, "a"), "one");
    assert_eq!(global_str(&e, "b"), "two\0still two");
    let _ = std::fs::remove_file(&path);
}

/// The blocking builtin (call_builtin, outside the evaluator's Class C
/// pull) frames by the handle's delimiter too. A frame longer than `max`
/// is refused even when its NUL arrives in the same read (the cap must not
/// depend on packet timing), and the refusal names NUL.
#[test]
fn blocking_recv_line_reads_nul_frames_and_caps_terminated_frames() {
    use mix::builtins::call_builtin;
    let path = sock_path("blk");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut out = b"first\0".to_vec();
        out.extend_from_slice(&[b'x'; 64]);
        out.push(0);
        s.write_all(&out).unwrap();
        std::thread::sleep(Duration::from_millis(500));
    });
    let mut opts = indexmap::IndexMap::new();
    opts.insert("frame".to_string(), Value::String("nul".into()));
    let h = call_builtin(
        "unix_connect",
        vec![
            Value::String(path.to_string_lossy().into_owned()),
            Value::map(opts),
        ],
    )
    .unwrap()
    .unwrap();
    let mut ropts = indexmap::IndexMap::new();
    ropts.insert("timeout".to_string(), Value::Number(5.0));
    ropts.insert("max".to_string(), Value::Number(16.0));
    let first = call_builtin("tcp_recv_line", vec![h.clone(), Value::map(ropts.clone())])
        .unwrap()
        .unwrap();
    assert_eq!(first.to_mix_string(), "first");
    std::thread::sleep(Duration::from_millis(200));
    let err = call_builtin("tcp_recv_line", vec![h.clone(), Value::map(ropts)]).unwrap_err();
    assert!(err.to_string().contains("before a NUL"), "{err}");
    server.join().unwrap();
    let _ = std::fs::remove_file(&path);
}

/// tcp_on {frame:"line"} on a NUL-framed unix handle delivers one event
/// per NUL frame, including a frame split across two writes.
#[tokio::test(flavor = "current_thread")]
async fn tcp_on_delivers_nul_frames_as_line_events() {
    let path = sock_path("on");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.write_all(b"{\"n\":1}\0{\"n\"").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        s.write_all(b":2}\0").unwrap();
        std::thread::sleep(Duration::from_millis(1000));
    });
    let (mut e, _tx) = served();
    e.set_global("path", Value::String(path.to_string_lossy().into_owned()));
    exec(
        &mut e,
        r#"
$h = unix_connect($path, {frame: "nul", timeout: 5})
$frames = []
tcp_on($h, "hostd.frame", {frame: "line"})
on hostd.frame
  if $event.args.frame != nil then
    push($frames, json_parse($event.args.frame.data).n)
    if length($frames) == 2 then quit() end
  end
end
"#,
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(60), e.run_event_pump())
        .await
        .expect("pump deadline")
        .unwrap();
    assert_eq!(
        e.get_global("frames").unwrap().to_mix_string(),
        Value::list(vec![Value::Number(1.0), Value::Number(2.0)]).to_mix_string()
    );
    server.join().unwrap();
    e.close_native_events();
    let _ = std::fs::remove_file(&path);
}

#[tokio::test(flavor = "current_thread")]
async fn refusals_carry_structured_codes() {
    // A socket file with no listener behind it: connect is refused.
    let dead = sock_path("dead");
    drop(UnixListener::bind(&dead).unwrap());
    let mut e = Evaluator::new();
    e.set_global("dead", Value::String(dead.to_string_lossy().into_owned()));
    e.set_global(
        "missing",
        Value::String(sock_path("missing").to_string_lossy().into_owned()),
    );
    exec(
        &mut e,
        r#"
fn code_of($f)
  try
    $f()
  catch $m, $err
    return $err.code
  end
  return "no error"
end
$missing_code = code_of(fn() unix_connect($missing) end)
$dead_code = code_of(fn() unix_connect($dead) end)
$relative = code_of(fn() unix_connect("run/hostd.sock") end)
$bad_frame = code_of(fn() unix_connect($dead, {frame: "crlf"}) end)
$bad_opt = code_of(fn() unix_connect($dead, {tls: true}) end)
$zero = code_of(fn() unix_connect($dead, {timeout: 0}) end)
$negative = code_of(fn() unix_connect($dead, {timeout: -1}) end)
$rounds_to_zero = code_of(fn() unix_connect($dead, {timeout: 0.0000000001}) end)
"#,
    )
    .await
    .unwrap();
    for (name, code) in [
        ("missing_code", "UNIX_CONNECT_NOT_FOUND"),
        ("dead_code", "UNIX_CONNECT_REFUSED"),
        ("relative", "UNIX_CONNECT_ARGUMENT"),
        ("bad_frame", "UNIX_CONNECT_ARGUMENT"),
        ("bad_opt", "UNIX_CONNECT_ARGUMENT"),
        ("zero", "UNIX_CONNECT_ARGUMENT"),
        ("negative", "UNIX_CONNECT_ARGUMENT"),
        ("rounds_to_zero", "UNIX_CONNECT_ARGUMENT"),
    ] {
        assert_eq!(global_str(&e, name), code, "${name}");
    }
    let _ = std::fs::remove_file(&dead);
}

/// A listener whose backlog is full: the connect fails at once with
/// UNIX_CONNECT_BUSY (AF_UNIX answers EAGAIN), never a generic code and
/// never a wait.
#[tokio::test(flavor = "current_thread")]
async fn a_full_backlog_is_busy_at_once() {
    use std::os::fd::AsRawFd;
    let path = sock_path("busy");
    let listener = UnixListener::bind(&path).unwrap();
    // Shrink the backlog to 0: Linux then queues one connection and
    // refuses the next with EAGAIN. Nothing ever accepts.
    // SAFETY: listen(2) on a valid listening socket fd.
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
    let mut e = Evaluator::new();
    e.set_global("path", Value::String(path.to_string_lossy().into_owned()));
    let started = std::time::Instant::now();
    exec(
        &mut e,
        r#"
$codes = []
$held = []
for $i = 1 to 8
  try
    push($held, unix_connect($path, {timeout: 5}))
    push($codes, "ok")
  catch $m, $err
    push($codes, $err.code)
  end
end
$busy = contains($codes, "UNIX_CONNECT_BUSY")
$other = false
for each $c in $codes
  if $c != "ok" and $c != "UNIX_CONNECT_BUSY" then $other = true end
end
"#,
    )
    .await
    .unwrap();
    assert_eq!(
        global_str(&e, "busy"),
        "true",
        "{}",
        global_str(&e, "codes")
    );
    assert_eq!(
        global_str(&e, "other"),
        "false",
        "{}",
        global_str(&e, "codes")
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "a busy connect waited"
    );
    drop(listener);
    let _ = std::fs::remove_file(&path);
}

/// The Class C pull caps a terminated frame by `max` too, and bytes it
/// read ahead (a second frame in the same read) carry into a later
/// tcp_on subscription as that subscription's first event.
#[tokio::test(flavor = "current_thread")]
async fn class_c_caps_terminated_frames_and_seeds_tcp_on() {
    let path = sock_path("seed");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        // Connection 1: one read holding two frames.
        let (mut a, _) = listener.accept().unwrap();
        a.write_all(b"alpha\0beta\0").unwrap();
        // Connection 2: an over-long frame whose NUL arrives with it.
        let (mut b, _) = listener.accept().unwrap();
        let mut long = vec![b'y'; 40];
        long.push(0);
        b.write_all(&long).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
    });
    let (mut e, tx) = served();
    e.set_global("path", Value::String(path.to_string_lossy().into_owned()));
    exec(
        &mut e,
        r#"
$first = nil
$capped = nil
$seeded = nil
-- An async handler: its numeric tcp_recv_line runs on the Class C pull.
on go async
  $a = unix_connect($path, {frame: "nul", timeout: 5})
  -- Let both frames land in one read before the first receive.
  sleep(0.2)
  $first = tcp_recv_line($a, {timeout: 5})
  $b = unix_connect($path, {frame: "nul", timeout: 5})
  sleep(0.2)
  try
    tcp_recv_line($b, {timeout: 5, max: 16})
    $capped = "delivered"
  catch $m, $err
    $capped = $err.code .. " " .. $m
  end
  tcp_on($a, "seed.frame", {frame: "line"})
end
on seed.frame
  if $event.args.frame != nil then
    $seeded = $event.args.frame.data
    quit()
  end
end
"#,
    )
    .await
    .unwrap();
    tx.send(mix::evaluator::IncomingEvent {
        generation: 0,
        command: "go".into(),
        body: "{}".into(),
        headers: std::collections::BTreeMap::new(),
    })
    .unwrap();
    // Async handlers spawn on the local task set, as in a served citizen.
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            tokio::time::timeout(Duration::from_secs(60), e.run_event_pump())
                .await
                .expect("pump deadline")
                .unwrap();
        })
        .await;
    assert_eq!(global_str(&e, "first"), "alpha");
    let capped = global_str(&e, "capped");
    assert!(
        capped.starts_with("SOCKET_CLOSED") && capped.contains("before a NUL"),
        "{capped}"
    );
    assert_eq!(global_str(&e, "seeded"), "beta");
    server.join().unwrap();
    e.close_native_events();
    let _ = std::fs::remove_file(&path);
}

/// tcp_on line mode caps a terminated over-long frame: one terminal event
/// naming NUL, and the frame is never delivered.
#[tokio::test(flavor = "current_thread")]
async fn tcp_on_caps_terminated_frames() {
    let path = sock_path("oncap");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut long = vec![b'z'; 40];
        long.push(0);
        s.write_all(&long).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
    });
    let (mut e, _tx) = served();
    e.set_global("path", Value::String(path.to_string_lossy().into_owned()));
    exec(
        &mut e,
        r#"
$h = unix_connect($path, {frame: "nul", timeout: 5})
$delivered = 0
$closed = nil
tcp_on($h, "cap.frame", {frame: "line", max: 16})
on cap.frame
  if $event.args.frame != nil then $delivered = $delivered + 1 end
  if $event.args.closed != nil then
    $closed = json_encode($event.args.closed)
    quit()
  end
end
"#,
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(60), e.run_event_pump())
        .await
        .expect("pump deadline")
        .unwrap();
    assert_eq!(global_str(&e, "delivered"), "0");
    assert!(
        global_str(&e, "closed").contains("before a NUL"),
        "{}",
        global_str(&e, "closed")
    );
    server.join().unwrap();
    e.close_native_events();
    let _ = std::fs::remove_file(&path);
}
