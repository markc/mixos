// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
#![cfg(feature = "http")]

use mix::evaluator::{Evaluator, SharedBuf};
use mix::lexer::Lexer;
use mix::parser::Parser;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

fn accept(listener: &TcpListener) -> (TcpStream, std::net::SocketAddr) {
    listener.set_nonblocking(true).unwrap();
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok(socket) => return socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(started.elapsed() < Duration::from_secs(2), "client did not connect");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept: {error}"),
        }
    }
}

fn server(f: impl FnOnce(TcpStream) + Send + 'static) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let task = thread::spawn(move || {
        let (mut socket, _) = accept(&listener);
        socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).unwrap(), 1);
            request.push(byte[0]);
        }
        let length = String::from_utf8_lossy(&request)
            .lines()
            .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok()))
            .unwrap_or(0);
        socket.read_exact(&mut vec![0; length]).unwrap();
        f(socket);
    });
    (url, task)
}

async fn run(source: &str) -> Result<String, mix::error::MixError> {
    let tokens = Lexer::new(source).tokenize()?;
    let stmts = Parser::new(tokens, source).parse_program()?;
    let output = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(output.clone()), Box::new(SharedBuf::new()));
    eval.execute(&stmts).await?;
    Ok(output.to_string_lossy())
}

async fn join(task: JoinHandle<()>) {
    // Let the current-thread runtime poll the HTTP connection's close path
    // while the server verifies EOF. A blocking join would park that runtime.
    tokio::task::spawn_blocking(move || task.join().unwrap()).await.unwrap();
}

fn sse_headers(socket: &mut TcpStream) {
    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
}

fn chunk(socket: &mut TcpStream, data: &[u8]) {
    write!(socket, "{:x}\r\n", data.len()).unwrap();
    socket.write_all(data).unwrap();
    socket.write_all(b"\r\n").unwrap();
}

#[tokio::test]
async fn healthy_sse_outlives_idle_budget_with_no_whole_call_cap() {
    // Fifteen intervals model the original 30 s stream without a 30 s test.
    let (url, task) = server(|mut socket| {
        sse_headers(&mut socket);
        for _ in 0..15 {
            chunk(&mut socket, b"data: ok\r\n\n");
            thread::sleep(Duration::from_millis(20));
        }
        socket.write_all(b"0\r\n\r\n").unwrap();
    });
    let source = format!(r#"
        $r = http_stream("GET", "{url}", fn($line) print($line) end,
                         nil, nil, {{timeout: 0, idle_timeout: 0.1}})
        print($r.status)
        print($r.bytes_received)
        print($r.error_code)
    "#);
    let out = run(&source).await.unwrap();
    join(task).await;
    assert_eq!(out, format!("{}200\n165\nnil\n", "data: ok\n\n".repeat(15)));
}

#[tokio::test]
async fn idle_failure_preserves_partial_line_and_byte_count() {
    let (url, task) = server(|mut socket| {
        sse_headers(&mut socket);
        chunk(&mut socket, b"data: first\npartial");
        thread::sleep(Duration::from_millis(250));
    });
    let out = run(&format!(r#"
        $r = http_stream("GET", "{url}", fn($line) print($line) end,
                         nil, nil, {{timeout: 0, idle_timeout: 0.1}})
        print($r.status)
        print($r.error_code)
        print($r.bytes_received)
        print($r.body)
    "#)).await.unwrap();
    join(task).await;
    assert_eq!(out, "data: first\n0\nHTTP_IDLE_TIMEOUT\n19\npartial\n");
}

#[tokio::test]
async fn byte_progress_resets_idle_budget_before_a_line_is_complete() {
    let (url, task) = server(|mut socket| {
        sse_headers(&mut socket);
        for byte in b"data: ok\n" {
            chunk(&mut socket, &[*byte]);
            thread::sleep(Duration::from_millis(20));
        }
        socket.write_all(b"0\r\n\r\n").unwrap();
    });
    let out = run(&format!(r#"
        $r = http_stream("GET", "{url}", fn($line) print($line) end,
                         nil, nil, {{timeout: 0, idle_timeout: 0.1}})
        print($r.status)
    "#)).await.unwrap();
    join(task).await;
    assert_eq!(out, "data: ok\n200\n");
}

#[tokio::test]
async fn early_stop_closes_without_waiting_for_next_line() {
    let (url, task) = server(|mut socket| {
        sse_headers(&mut socket);
        chunk(&mut socket, b"first\n");
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0, "client must close promptly");
    });
    let started = Instant::now();
    let out = run(&format!(r#"
        $r = http_stream("GET", "{url}", fn($line) = false,
                         nil, nil, {{timeout: 0, idle_timeout: 10}})
        print($r.status)
        print($r.stopped)
    "#)).await.unwrap();
    join(task).await;
    assert_eq!(out, "200\ntrue\n");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn callback_error_closes_the_response() {
    let (url, task) = server(|mut socket| {
        sse_headers(&mut socket);
        chunk(&mut socket, b"first\n");
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    let error = run(&format!(r#"
        $r = http_stream("GET", "{url}", fn($line) raise("TEST", "callback") end)
    "#)).await.unwrap_err();
    assert!(error.to_string().contains("callback"));
    join(task).await;
}

#[tokio::test]
async fn premature_content_length_and_chunked_eof_are_values() {
    for framing in ["Content-Length: 100", "Transfer-Encoding: chunked"] {
        let (url, task) = server(move |mut socket| {
            write!(socket, "HTTP/1.1 200 OK\r\n{framing}\r\nConnection: close\r\n\r\n").unwrap();
            if framing.starts_with("Content-Length") {
                socket.write_all(b"first\npart").unwrap();
            } else {
                chunk(&mut socket, b"first\npart");
                // No final zero chunk.
            }
        });
        let out = run(&format!(r#"
            $r = http_stream("GET", "{url}", fn($line) print($line) end)
            print($r.status)
            print($r.error_code)
            print($r.bytes_received)
            print($r.body)
        "#)).await.unwrap();
        join(task).await;
        assert_eq!(out, "first\n0\nHTTP_STREAM_CUT\n10\npart\n");
    }
}

#[tokio::test]
async fn buffered_calls_keep_partial_bytes_on_idle_timeout() {
    for call in ["http_get", "http_post", "http_request"] {
        let (url, task) = server(|mut socket| {
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial").unwrap();
            thread::sleep(Duration::from_millis(250));
        });
        let args = match call {
            "http_post" => format!("\"{url}\", \"\""),
            "http_request" => format!("\"GET\", \"{url}\", nil"),
            _ => format!("\"{url}\""),
        };
        let out = run(&format!(r#"
            $r = {call}({args}, nil, {{timeout: 0, idle_timeout: 0.1}})
            print($r.status)
            print($r.error_code)
            print($r.body)
            print($r.bytes_received)
        "#)).await.unwrap();
        join(task).await;
        assert_eq!(out, "0\nHTTP_IDLE_TIMEOUT\npartial\n7\n", "{call}");
    }
}

#[tokio::test]
async fn ordinary_calls_keep_their_status_body_and_error_semantics() {
    for call in ["http_get", "http_post", "http_request"] {
        let (url, task) = server(|mut socket| {
            socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\n\r\noops").unwrap();
        });
        let args = match call {
            "http_post" => format!("\"{url}\", \"\""),
            "http_request" => format!("\"GET\", \"{url}\""),
            _ => format!("\"{url}\""),
        };
        assert_eq!(run(&format!(r#"
            $r = {call}({args})
            print($r.status)
            print($r.body)
            print($r.error_code)
        "#)).await.unwrap(), "404\noops\nnil\n");
        join(task).await;
    }
}

#[tokio::test]
async fn final_unterminated_line_and_blank_lines_are_delivered() {
    let (url, task) = server(|mut socket| {
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n\r\nlast!").unwrap();
    });
    assert_eq!(run(&format!(r#"
        $r = http_stream("GET", "{url}", fn($line) print("[" .. $line .. "]") end)
        print($r.body)
    "#)).await.unwrap(), "[]\n[last!]\n\n");
    join(task).await;
}

#[tokio::test]
async fn whole_call_cap_still_bounds_a_healthy_stream() {
    let (url, task) = server(|mut socket| {
        sse_headers(&mut socket);
        for _ in 0..80 {
            if socket.write_all(b"2\r\nx\n\r\n").is_err() { return; }
            thread::sleep(Duration::from_millis(20));
        }
    });
    let out = run(&format!(r#"
        $r = http_stream("GET", "{url}", fn($line) = true,
                         nil, nil, {{timeout: 1, idle_timeout: 0.2}})
        print($r.status)
        print($r.error_code)
        print($r.bytes_received > 0)
    "#)).await.unwrap();
    join(task).await;
    assert_eq!(out, "0\nHTTP_TIMEOUT\ntrue\n");
}

#[tokio::test]
async fn cancellation_drops_a_blocked_response() {
    let (url, task) = server(|mut socket| {
        sse_headers(&mut socket);
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    let source = format!(r#"
        $r = http_stream("GET", "{url}", fn($line) = true,
                         nil, nil, {{timeout: 0, idle_timeout: 10}})
    "#);
    assert!(tokio::time::timeout(Duration::from_millis(100), run(&source)).await.is_err());
    join(task).await;
}

#[tokio::test]
async fn evaluator_interrupt_wakes_a_blocked_read() {
    let (url, task) = server(|mut socket| {
        sse_headers(&mut socket);
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    let source = format!(r#"
        $r = http_stream("GET", "{url}", fn($line) = true,
                         nil, nil, {{timeout: 0, idle_timeout: 10}})
    "#);
    let tokens = Lexer::new(&source).tokenize().unwrap();
    let stmts = Parser::new(tokens, &source).parse_program().unwrap();
    let mut eval = Evaluator::new();
    let flag = eval.interrupt_flag();
    let raise = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        flag.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    // The builtin returns HTTP_INTERRUPTED, and the evaluator's next
    // checkpoint may turn the still-standing flag into its unwind error.
    let result = tokio::time::timeout(Duration::from_secs(2), eval.execute(&stmts)).await;
    assert!(result.is_ok(), "interrupt must beat the 10 s idle limit");
    raise.await.unwrap();
    join(task).await;
}

#[tokio::test]
async fn network_and_conditional_ca_file_capabilities_are_gated_before_io() {
    for (source, caps, expected) in [
        (
            "$r = http_stream(\"GET\", \"http://127.0.0.1:1/\", fn($line) = true)",
            vec![],
            "Network capability not allowed",
        ),
        (
            "$r = http_stream(\"GET\", \"http://127.0.0.1:1/\", fn($line) = true, nil, nil, {ca_file: \"absent.pem\"})",
            vec![mix::CapabilityClass::Network],
            "FsRead capability not allowed",
        ),
        (
            "$r = http_get(\"http://127.0.0.1:1/\", {ca_file: \"absent.pem\", idle_timeout: 1})",
            vec![mix::CapabilityClass::Network],
            "FsRead capability not allowed",
        ),
    ] {
        let tokens = Lexer::new(source).tokenize().unwrap();
        let stmts = Parser::new(tokens, source).parse_program().unwrap();
        let mut eval = Evaluator::new();
        eval.set_capability_policy(std::rc::Rc::new(mix::CategoryAllowList::new(&caps)));
        let error = eval.execute(&stmts).await.unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    }
}

#[tokio::test]
async fn invalid_idle_options_raise_before_network_io() {
    for idle in ["0", "-1", "nil", "true", "31536001"] {
        let error = run(&format!(r#"
            $r = http_get("http://127.0.0.1:1/", {{idle_timeout: {idle}}})
        "#)).await.unwrap_err();
        assert!(error.to_string().contains("idle_timeout"));
    }
}

#[tokio::test]
async fn tls_verification_and_explicit_insecure_mode_match_buffered_http() {
    use ureq::rustls;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    for insecure in [false, true] {
        let cert = CertificateDer::from_pem_slice(include_bytes!("fixtures/socket-test-cert.pem")).unwrap();
        let key = PrivateKeyDer::from_pem_slice(include_bytes!("fixtures/socket-test-key.pem")).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(
            std::sync::Arc::new(rustls::crypto::ring::default_provider()),
        ).with_safe_default_protocol_versions().unwrap()
            .with_no_client_auth().with_single_cert(vec![cert], key).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("https://{}/", listener.local_addr().unwrap());
        let task = thread::spawn(move || {
            let (socket, _) = accept(&listener);
            socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut socket = rustls::StreamOwned::new(
                rustls::ServerConnection::new(std::sync::Arc::new(config)).unwrap(), socket,
            );
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                match socket.read(&mut byte) {
                    Ok(1) => request.push(byte[0]),
                    _ if !insecure => return,
                    other => panic!("TLS request failed: {other:?}"),
                }
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nok\n").unwrap();
            socket.flush().unwrap();
        });
        let out = run(&format!(r#"
            $r = http_stream("GET", "{url}", fn($line) print($line) end,
                             nil, nil, {{ssl_verify: {verify}}})
            print($r.status)
        "#, verify = !insecure)).await.unwrap();
        join(task).await;
        assert_eq!(out, if insecure { "ok\n200\n" } else { "0\n" });
    }
}

#[tokio::test]
async fn post_redirect_does_not_replay_a_body_on_307() {
    let (url, task) = server(|mut socket| {
        socket.write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /other\r\nContent-Length: 0\r\n\r\n").unwrap();
    });
    let out = run(&format!(r#"
        $r = http_stream("POST", "{url}", fn($line) = true, "body")
        print($r.status)
    "#)).await.unwrap();
    join(task).await;
    assert_eq!(out, "307\n");
}

#[tokio::test]
async fn get_redirects_and_credential_stripping_match_the_existing_path() {
    for path in ["ordinary", "idle", "stream"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = thread::spawn(move || {
            for redirect in [true, false] {
                let (mut socket, _) = accept(&listener);
                socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    assert_eq!(socket.read(&mut byte).unwrap(), 1);
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                if redirect {
                    assert!(request.contains("authorization: test"));
                    socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                } else {
                    assert!(request.starts_with("get /final "));
                    assert!(!request.contains("authorization:"));
                    assert!(!request.contains("cookie:"));
                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nok\n").unwrap();
                }
            }
        });
        let headers = r#"{Authorization: "test", Cookie: "test=1"}"#;
        let call = match path {
            "ordinary" => format!("http_get(\"{url}\", {headers})"),
            "idle" => format!("http_get(\"{url}\", {headers}, {{idle_timeout: 1}})"),
            _ => format!("http_stream(\"GET\", \"{url}\", fn($line) = true, nil, {headers})"),
        };
        let out = run(&format!(r#"
            $r = {call}
            print($r.status)
            print(ends_with($r.final_url, "/final"))
        "#)).await.unwrap();
        join(task).await;
        assert_eq!(out, "200\ntrue\n", "{path}");
    }
}

#[tokio::test]
async fn callback_deadline_does_not_abandon_an_evaluator_function_frame() {
    let (url, task) = server(|mut socket| {
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nok\n").unwrap();
    });
    let out = run(&format!(r#"
        $sentinel = "outer"
        $r = http_stream("GET", "{url}", fn($line)
            $sentinel = "inner"
            sleep(1.1)
        end, nil, nil, {{timeout: 1}})
        print($sentinel)
        print($r.error_code)
    "#)).await.unwrap();
    join(task).await;
    assert_eq!(out, "outer\nHTTP_TIMEOUT\n");
}
