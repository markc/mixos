// SPDX-License-Identifier: MIT OR Apache-2.0
use mix::{MixError, value::Value};
use mix_server::*;
fn builtin_udp_send(args: Vec<Value>) -> mix::MixResult<Option<Value>> { mix::builtins::call_builtin("udp_send", args) }
fn tmpdir(_: &str) -> std::path::PathBuf { tempfile::tempdir().unwrap().keep() }
fn s(p: &std::path::Path) -> Value { Value::String(p.to_string_lossy().into()) }
fn opts(pairs: &[(&str, Value)]) -> Value { Value::map(pairs.iter().map(|(k,v)| ((*k).into(), v.clone())).collect()) }
fn is_type_mismatch(err: &MixError) -> bool { err.info().is_some_and(|e| e.code == "TYPE_MISMATCH") }
    #[test]
    fn udp_loopback_and_timeout() {
        // Find a free port, release it, and race to rebind — acceptable in
        // a test; the datagram goes over loopback only.
        let port = {
            let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            s.local_addr().unwrap().port()
        };
        // Value is Rc-based (not Send), so the sender thread builds its own
        // argument Values from Copy/'static inputs. It RETRIES for the whole
        // window: under a parallel workspace test run the receiver may bind
        // arbitrarily late, and a datagram sent before the bind is silently
        // dropped — one timed send is a flake, a resend loop is not. The
        // receiver reads exactly one datagram; extras evaporate with the
        // socket.
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_tx = stop.clone();
        let sender = std::thread::spawn(move || {
            let mut first: Option<usize> = None;
            for _ in 0..100 {
                if stop_tx.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let sent = builtin_udp_send(vec![
                    Value::String("127.0.0.1".into()),
                    Value::Number(port as f64),
                    Value::String("ping-π".into()),
                ]);
                match sent {
                    Ok(Some(Value::Number(n))) => first.get_or_insert(n as usize),
                    other => return Err(format!("{other:?}")),
                };
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            first.ok_or_else(|| "sender never ran".to_string())
        });
        let mut opts = indexmap::IndexMap::new();
        opts.insert("timeout".to_string(), Value::Number(5.0));
        opts.insert("host".to_string(), Value::String("127.0.0.1".into()));
        let got = builtin_udp_recv(vec![Value::Number(port as f64), Value::map(opts)])
            .unwrap()
            .unwrap();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let sent = sender.join().unwrap().unwrap();
        assert_eq!(sent, "ping-π".len());
        let Value::Map(m) = &got else {
            panic!("{got:?}")
        };
        assert_eq!(m.get("text"), Some(&Value::String("ping-π".into())));
        assert_eq!(m.get("from_host"), Some(&Value::String("127.0.0.1".into())));
        match m.get("bytes") {
            Some(Value::Bytes(b)) => assert_eq!(b.as_slice(), "ping-π".as_bytes()),
            other => panic!("{other:?}"),
        }

        // Timeout returns nil, not an error.
        let port2 = {
            let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            s.local_addr().unwrap().port()
        };
        let mut opts = indexmap::IndexMap::new();
        opts.insert("timeout".to_string(), Value::Number(0.1));
        opts.insert("host".to_string(), Value::String("127.0.0.1".into()));
        assert_eq!(
            builtin_udp_recv(vec![Value::Number(port2 as f64), Value::map(opts)]).unwrap(),
            Some(Value::Nil)
        );
    }

    #[test]
    fn http_serve_static_round_trip_and_rules() {
        let dir = std::env::temp_dir().join(format!("mix-http-serve-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("hello.txt"), "hi there\n").unwrap();
        std::fs::write(dir.join("sub/page.html"), "<h1>ok</h1>").unwrap();
        // The escape targets: a real file OUTSIDE the root, and a symlink
        // INSIDE the root pointing at it.
        let outside = std::env::temp_dir().join(format!("mix-http-secret-{}", std::process::id()));
        std::fs::write(&outside, "SECRET").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("leak")).unwrap();

        let port = free_port();
        let root = dir.to_string_lossy().to_string();
        let server = std::thread::spawn(move || {
            let mut opts = indexmap::IndexMap::new();
            opts.insert("port".to_string(), Value::Number(port as f64));
            opts.insert("requests".to_string(), Value::Number(7.0));
            opts.insert("listing".to_string(), Value::Bool(true));
            match builtin_http_serve(vec![Value::String(root), Value::map(opts)]) {
                Ok(Some(Value::Number(n))) => Ok(n as u64),
                other => Err(format!("{other:?}")),
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(150));

        // 1. Plain file with the right type and body.
        let r = http_raw(port, "GET /hello.txt HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        assert!(r.contains("text/plain"), "{r}");
        assert!(r.ends_with("hi there\n"), "{r}");
        // 2. Nested file.
        let r = http_raw(port, "GET /sub/page.html HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            r.starts_with("HTTP/1.1 200") && r.contains("text/html"),
            "{r}"
        );
        // 3. TRAVERSAL, plain: verbatim ../ on the wire never reaches the
        // secret (404: the canonicalised path resolves outside → prefix
        // check, but the join may also simply not exist — either way the
        // body must not leak).
        let r = http_raw(port, "GET /../mix-http-secret HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(!r.contains("SECRET"), "{r}");
        assert!(
            r.starts_with("HTTP/1.1 403") || r.starts_with("HTTP/1.1 404"),
            "{r}"
        );
        // 4. TRAVERSAL, percent-encoded: %2e%2e%2f decodes to ../ BEFORE
        // the check — the gate must fail this too, provably.
        let r = http_raw(
            port,
            "GET /%2e%2e%2fmix-http-secret HTTP/1.1\r\nHost: x\r\n\r\n",
        );
        assert!(!r.contains("SECRET"), "{r}");
        // 5. Symlink escape: exists, resolves outside root → 403 exactly.
        let r = http_raw(port, "GET /leak HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            r.starts_with("HTTP/1.1 403"),
            "symlink escape must 403: {r}"
        );
        assert!(!r.contains("SECRET"), "{r}");
        // 6. Non-GET is 405 with Allow.
        let r = http_raw(
            port,
            "POST /hello.txt HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
        );
        assert!(
            r.starts_with("HTTP/1.1 405") && r.contains("Allow: GET, HEAD"),
            "{r}"
        );
        // 7. Directory listing (opted in) links both entries.
        let r = http_raw(port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(r.contains("hello.txt") && r.contains("sub/"), "{r}");

        assert_eq!(server.join().unwrap().unwrap(), 7);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_file(&outside).ok();
    }

    #[test]
    fn http_serve_spa_fallback() {
        let dir = std::env::temp_dir().join(format!("mix-http-spa-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html><div id=app></div>").unwrap();
        std::fs::write(dir.join("app.js"), "console.log('router')").unwrap();
        let port = free_port();
        let root = dir.to_string_lossy().to_string();
        let server = std::thread::spawn(move || {
            let mut opts = indexmap::IndexMap::new();
            opts.insert("port".to_string(), Value::Number(port as f64));
            opts.insert("requests".to_string(), Value::Number(5.0));
            opts.insert("spa".to_string(), Value::Bool(true));
            builtin_http_serve(vec![Value::String(root), Value::map(opts)])
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        std::thread::sleep(std::time::Duration::from_millis(150));
        // 1. A real asset still serves as itself.
        let r = http_raw(port, "GET /app.js HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 200") && r.contains("router"), "{r}");
        // 2. A deep extensionless route the router owns → the shell, 200.
        let r = http_raw(port, "GET /bus/props-core HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            r.starts_with("HTTP/1.1 200") && r.contains("id=app"),
            "deep route → shell: {r}"
        );
        // 3. A MISSING ASSET must stay a real 404, never the shell (else a
        //    broken /main.css silently becomes HTML and the app breaks
        //    confusingly).
        let r = http_raw(port, "GET /missing.css HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            r.starts_with("HTTP/1.1 404"),
            "missing asset stays 404: {r}"
        );
        assert!(!r.contains("id=app"), "{r}");
        // 4. A traversal attempt decodes to an extensionless path
        //    (`../etc`), so it DOES receive the shell (200) — but the shell
        //    is a startup-vetted constant inside the root, so no file
        //    outside the root is ever read. That is the safety property:
        //    the fallback serves one known file, never a request-derived
        //    path. The traversal gate itself (proven in the main test) is
        //    untouched; this just confirms the shell can't leak.
        let r = http_raw(port, "GET /%2e%2e%2fetc HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 200") && r.contains("id=app"), "{r}");
        assert!(!r.contains("root:") && !r.contains("SECRET"), "{r}");
        // 5. The root still serves index directly.
        let r = http_raw(port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 200") && r.contains("id=app"), "{r}");
        server.join().unwrap().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// clean_urls (GitHub Pages parity): /foo serves /foo.html when the
    /// pre-rendered page exists — the fix for a stub/page-per-route site
    /// like mixos.dev, distinct from and tried before the SPA shell.
    #[test]
    fn http_serve_clean_urls_resolves_dot_html() {
        let dir = std::env::temp_dir().join(format!("mix-http-clean-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bus")).unwrap();
        std::fs::write(dir.join("bus/props-core.html"), "<h1>props-core page</h1>").unwrap();
        std::fs::write(dir.join("plain.txt"), "text").unwrap();
        let port = free_port();
        let root = dir.to_string_lossy().to_string();
        let server = std::thread::spawn(move || {
            let mut opts = indexmap::IndexMap::new();
            opts.insert("port".to_string(), Value::Number(port as f64));
            opts.insert("requests".to_string(), Value::Number(4.0));
            opts.insert("clean_urls".to_string(), Value::Bool(true));
            builtin_http_serve(vec![Value::String(root), Value::map(opts)])
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        std::thread::sleep(std::time::Duration::from_millis(150));
        // Clean URL → the .html page, styled/served as HTML.
        let r = http_raw(port, "GET /bus/props-core HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            r.starts_with("HTTP/1.1 200")
                && r.contains("props-core page")
                && r.contains("text/html"),
            "{r}"
        );
        // The explicit .html still works.
        let r = http_raw(port, "GET /bus/props-core.html HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        // A path WITH an extension is never .html-appended (no /plain.txt.html).
        let r = http_raw(port, "GET /plain.txt HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            r.starts_with("HTTP/1.1 200") && r.contains("text/plain"),
            "{r}"
        );
        // A genuinely missing clean URL is still a 404 (no shell configured).
        let r = http_raw(port, "GET /bus/nope HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 404"), "{r}");
        server.join().unwrap().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The MAJOR the review caught: a section PAGE (`bus.html`) sitting
    /// beside its children DIRECTORY (`bus/`) — `GET /bus` must resolve to
    /// the page, not 403 because the directory exists with no index. Pages
    /// parity for the natural page-per-route layout.
    #[test]
    fn http_serve_clean_urls_sibling_page_beside_dir() {
        let dir = std::env::temp_dir().join(format!("mix-http-sibling-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bus")).unwrap();
        std::fs::write(dir.join("bus.html"), "<h1>bus section</h1>").unwrap();
        std::fs::write(dir.join("bus/props.html"), "<h1>child</h1>").unwrap();
        let port = free_port();
        let root = dir.to_string_lossy().to_string();
        let server = std::thread::spawn(move || {
            let mut opts = indexmap::IndexMap::new();
            opts.insert("port".to_string(), Value::Number(port as f64));
            opts.insert("requests".to_string(), Value::Number(3.0));
            opts.insert("clean_urls".to_string(), Value::Bool(true));
            builtin_http_serve(vec![Value::String(root), Value::map(opts)])
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        std::thread::sleep(std::time::Duration::from_millis(150));
        // /bus (the dir exists, no index) resolves to the sibling bus.html.
        let r = http_raw(port, "GET /bus HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            r.starts_with("HTTP/1.1 200") && r.contains("bus section"),
            "sibling page beside dir: {r}"
        );
        // /bus/props still resolves to the child.
        let r = http_raw(port, "GET /bus/props HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 200") && r.contains("child"), "{r}");
        // /bus/ (trailing slash) is a directory request → 403 (no index,
        // no listing), NOT the sibling page.
        let r = http_raw(port, "GET /bus/ HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(
            r.starts_with("HTTP/1.1 403"),
            "trailing slash is a dir request: {r}"
        );
        server.join().unwrap().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn http_serve_spa_bad_shell_fails_at_startup() {
        let dir = std::env::temp_dir().join(format!("mix-http-spa-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut opts = indexmap::IndexMap::new();
        opts.insert("port".to_string(), Value::Number(free_port() as f64));
        opts.insert("spa".to_string(), Value::String("nope.html".into()));
        let err = builtin_http_serve(vec![
            Value::String(dir.to_string_lossy().into_owned()),
            Value::map(opts),
        ])
        .unwrap_err();
        assert!(format!("{err}").contains("spa shell"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn http_serve_head_has_headers_but_no_body() {
        let dir = std::env::temp_dir().join(format!("mix-http-head-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "0123456789").unwrap();
        let port = free_port();
        let root = dir.to_string_lossy().to_string();
        let server = std::thread::spawn(move || {
            let mut opts = indexmap::IndexMap::new();
            opts.insert("port".to_string(), Value::Number(port as f64));
            opts.insert("requests".to_string(), Value::Number(1.0));
            builtin_http_serve(vec![Value::String(root), Value::map(opts)])
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        std::thread::sleep(std::time::Duration::from_millis(150));
        let r = http_raw(port, "HEAD /f.txt HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        assert!(r.contains("Content-Length: 10"), "{r}");
        assert!(r.ends_with("\r\n\r\n"), "HEAD must carry no body: {r:?}");
        server.join().unwrap().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The parser's refusals, each pinned over a raw socket: chunked is
    /// 501 (never a silent 200 with a dropped body), duplicate
    /// Content-Length is 400, a caret-less endless line is 431, and a
    /// POST with a body gets serve's honest 405 (method gate before any
    /// body concern). Plus: CRLF in a respond header raises locally
    /// before anything binds.
    #[test]
    fn http_parser_refusals_are_pinned() {
        let dir = std::env::temp_dir().join(format!("mix-http-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "x").unwrap();
        let port = free_port();
        let root = dir.to_string_lossy().to_string();
        let server = std::thread::spawn(move || {
            let mut opts = indexmap::IndexMap::new();
            opts.insert("port".to_string(), Value::Number(port as f64));
            opts.insert("requests".to_string(), Value::Number(4.0));
            builtin_http_serve(vec![Value::String(root), Value::map(opts)])
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        std::thread::sleep(std::time::Duration::from_millis(150));
        let r = http_raw(
            port,
            "POST /f.txt HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n",
        );
        assert!(r.starts_with("HTTP/1.1 501"), "chunked must 501: {r}");
        let r = http_raw(
            port,
            "GET /f.txt HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nZ",
        );
        assert!(r.starts_with("HTTP/1.1 400"), "dup CL must 400: {r}");
        let r = http_raw(
            port,
            "POST /f.txt HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello",
        );
        assert!(
            r.starts_with("HTTP/1.1 405"),
            "POST with body gets the method gate, not 413: {r}"
        );
        let long = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(20_000));
        let r = http_raw(port, &long);
        assert!(r.starts_with("HTTP/1.1 431"), "endless line must 431: {r}");
        server.join().unwrap().unwrap();
        std::fs::remove_dir_all(&dir).ok();

        // respond-splitting refusal is local and immediate.
        let mut respond = indexmap::IndexMap::new();
        let mut h = indexmap::IndexMap::new();
        h.insert(
            "X-Trace".to_string(),
            Value::String("a\r\nSet-Cookie: owned=1".into()),
        );
        respond.insert("headers".to_string(), Value::map(h));
        let mut opts = indexmap::IndexMap::new();
        opts.insert("respond".to_string(), Value::map(respond));
        opts.insert("timeout".to_string(), Value::Number(0.1));
        let err = builtin_http_recv(vec![Value::Number(free_port() as f64), Value::map(opts)])
            .unwrap_err();
        assert!(format!("{err}").contains("response splitting"), "{err}");
    }

    #[test]
    fn http_recv_catches_one_request_and_times_out_clean() {
        let port = free_port();
        let client = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            http_raw(
                port,
                "POST /hook?run=42 HTTP/1.1\r\nHost: x\r\nX-Token: abc\r\nContent-Length: 9\r\n\r\npayload=1",
            )
        });
        let mut opts = indexmap::IndexMap::new();
        opts.insert("timeout".to_string(), Value::Number(5.0));
        let mut respond = indexmap::IndexMap::new();
        respond.insert("status".to_string(), Value::Number(202.0));
        respond.insert("body".to_string(), Value::String("queued\n".into()));
        opts.insert("respond".to_string(), Value::map(respond));
        let got = builtin_http_recv(vec![Value::Number(port as f64), Value::map(opts)])
            .unwrap()
            .unwrap();
        let Value::Map(m) = &got else {
            panic!("{got:?}")
        };
        assert_eq!(m.get("method"), Some(&Value::String("POST".into())));
        assert_eq!(m.get("path"), Some(&Value::String("/hook".into())));
        assert_eq!(m.get("query"), Some(&Value::String("run=42".into())));
        assert_eq!(m.get("body"), Some(&Value::String("payload=1".into())));
        let Some(Value::Map(h)) = m.get("headers") else {
            panic!()
        };
        assert_eq!(h.get("x-token"), Some(&Value::String("abc".into())));
        // The client saw the configured response.
        let resp = client.join().unwrap();
        assert!(resp.starts_with("HTTP/1.1 202"), "{resp}");
        assert!(resp.ends_with("queued\n"), "{resp}");

        // And a quiet port answers nil, not an error.
        let port2 = free_port();
        let mut opts = indexmap::IndexMap::new();
        opts.insert("timeout".to_string(), Value::Number(0.2));
        assert_eq!(
            builtin_http_recv(vec![Value::Number(port2 as f64), Value::map(opts)]).unwrap(),
            Some(Value::Nil)
        );
    }

    #[test]
    fn http_serve_bool_options_refuse_before_bind() {
        let d = tmpdir("http");
        // Validation happens before bind/listen, so a wrong bool returns an
        // error immediately rather than starting a server.
        for (key, bad) in [
            ("listing", Value::String("false".into())),
            ("render_md", Value::Number(1.0)),
            ("clean_urls", Value::Nil),
        ] {
            let err = builtin_http_serve(vec![s(&d), opts(&[(key, bad)])]).unwrap_err();
            assert!(is_type_mismatch(&err), "{err:?}");
            assert!(format!("{err}").contains(key), "{err}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Raw-socket exchange helper: writes the request VERBATIM (a std
    /// HTTP client would normalize `../` away — an attacker's socket
    /// does not) and returns the whole response.
    fn http_raw(port: u16, request: &str) -> String {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        // A refusing server may close with our bytes still unread, which
        // surfaces as EPIPE on write and ECONNRESET on read — both are
        // normal for the refusal cases; keep whatever arrived.
        let _ = s.write_all(request.as_bytes());
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

