// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
pub mod http_srv {
    use std::io::{BufRead, BufReader, Write};

    pub struct RequestHead {
        pub method: String,
        pub target: String,
        pub headers: indexmap::IndexMap<String, String>,
        pub content_length: usize,
    }

    /// Read one CRLF line INCREMENTALLY: the cap and the deadline are
    /// enforced per chunk, never after the fact — a caller streaming
    /// gigabytes with no newline is refused at `cap`, and a byte-drip
    /// slowloris is refused at `deadline`, whatever the per-read socket
    /// timeout is (http_accept_next sets a 1 s tick so this loop
    /// actually gets to look at the clock). Returns the line WITHOUT
    /// its terminator; invalid UTF-8 is the caller's 400, not a
    /// mislabelled timeout.
    pub fn read_line_capped(
        reader: &mut BufReader<&mut std::net::TcpStream>,
        cap: usize,
        deadline: std::time::Instant,
    ) -> Result<String, (u16, &'static str)> {
        let mut line: Vec<u8> = Vec::new();
        loop {
            if std::time::Instant::now() >= deadline {
                return Err((408, "request read deadline exceeded"));
            }
            let buf = match reader.fill_buf() {
                Ok(b) => b,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue; // tick: the deadline check above governs
                }
                Err(_) => return Err((408, "connection error while reading")),
            };
            if buf.is_empty() {
                return Err((400, "connection closed mid-request"));
            }
            let (consumed, done) = match buf.iter().position(|&b| b == b'\n') {
                Some(i) => (i + 1, true),
                None => (buf.len(), false),
            };
            if line.len() + consumed > cap {
                return Err((431, "line exceeds the size cap"));
            }
            line.extend_from_slice(&buf[..consumed]);
            reader.consume(consumed);
            if done {
                while matches!(line.last(), Some(b'\n' | b'\r')) {
                    line.pop();
                }
                return String::from_utf8(line).map_err(|_| (400, "non-UTF-8 in request"));
            }
        }
    }

    /// Parse the request line + headers (no body). One whole-head
    /// deadline bounds the read however many lines arrive. Refusals the
    /// head itself decides: duplicate or malformed Content-Length (400,
    /// RFC 7230 §5.3), obs-fold continuations (400, §3.2.4), any
    /// Transfer-Encoding (501 — chunked is deliberately unsupported, and
    /// silently dropping a chunked body while answering 200 is the one
    /// thing a webhook catcher must never do).
    pub fn read_head(
        reader: &mut BufReader<&mut std::net::TcpStream>,
        deadline: std::time::Instant,
    ) -> Result<RequestHead, (u16, &'static str)> {
        let line = read_line_capped(reader, 8192, deadline)?;
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();
        let version = parts.next().unwrap_or("");
        if method.is_empty() || !target.starts_with('/') || !version.starts_with("HTTP/1") {
            return Err((400, "malformed request line"));
        }
        let mut headers = indexmap::IndexMap::new();
        let mut header_bytes = 0usize;
        loop {
            let h = read_line_capped(reader, 8192, deadline)?;
            if h.is_empty() {
                break;
            }
            header_bytes += h.len();
            if header_bytes > 16384 {
                return Err((431, "headers too large"));
            }
            if h.starts_with(' ') || h.starts_with('\t') {
                return Err((400, "obsolete line folding not supported"));
            }
            let Some((k, v)) = h.split_once(':') else {
                return Err((400, "malformed header line"));
            };
            let key = k.trim().to_ascii_lowercase();
            let prev = headers.insert(key.clone(), v.trim().to_string());
            if prev.is_some() && key == "content-length" {
                return Err((400, "duplicate content-length"));
            }
        }
        if headers.contains_key("transfer-encoding") {
            return Err((501, "transfer-encoding not supported"));
        }
        let content_length = match headers.get("content-length") {
            None => 0,
            Some(cl) => {
                if cl.is_empty() || !cl.bytes().all(|b| b.is_ascii_digit()) {
                    return Err((400, "malformed content-length"));
                }
                cl.parse().map_err(|_| (400, "malformed content-length"))?
            }
        };
        Ok(RequestHead {
            method,
            target,
            headers,
            content_length,
        })
    }

    /// Read the declared body (caller has already gated method/size
    /// policy) under the same deadline discipline.
    pub fn read_body(
        reader: &mut BufReader<&mut std::net::TcpStream>,
        len: usize,
        deadline: std::time::Instant,
    ) -> Result<Vec<u8>, (u16, &'static str)> {
        let mut body: Vec<u8> = Vec::with_capacity(len.min(64 * 1024));
        while body.len() < len {
            if std::time::Instant::now() >= deadline {
                return Err((408, "request read deadline exceeded"));
            }
            let buf = match reader.fill_buf() {
                Ok(b) => b,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(_) => return Err((408, "connection error while reading body")),
            };
            if buf.is_empty() {
                return Err((400, "connection closed mid-body"));
            }
            let take = buf.len().min(len - body.len());
            body.extend_from_slice(&buf[..take]);
            reader.consume(take);
        }
        Ok(body)
    }

    /// Percent-decode a PATH component: `%XX` only — no `+`→space (that
    /// is the form-urlencoded rule; in a path, `+` is a literal plus,
    /// and decoding it breaks every `a+b.txt` on disk).
    pub fn percent_decode_path(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%'
                && let (Some(h), Some(l)) = (
                    bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16)),
                    bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16)),
                )
            {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Percent-encoding for a listing href: keep it a working relative
    /// link whatever the filename holds. Unreserved ASCII passes;
    /// everything else — including `&` (a browser entity-decodes hrefs)
    /// and every non-ASCII char — is percent-encoded per UTF-8 BYTE (a
    /// `u8 as char` passthrough would Latin-1-mangle multibyte names
    /// into dead links; the re-review's catch).
    pub fn href_encode(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'_'
                | b'.'
                | b'~'
                | b'+'
                | b'@'
                | b','
                | b'='
                | b'('
                | b')' => out.push(b as char),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    pub fn reason(status: u16) -> &'static str {
        match status {
            200 => "OK",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            408 => "Request Timeout",
            413 => "Payload Too Large",
            414 => "URI Too Long",
            431 => "Request Header Fields Too Large",
            500 => "Internal Server Error",
            _ => "",
        }
    }

    /// Write a full response (or headers only for HEAD). Connection:
    /// close always — no keep-alive, one exchange per connection.
    pub fn write_response(
        stream: &mut std::net::TcpStream,
        status: u16,
        content_type: &str,
        extra_headers: &[(String, String)],
        body: &[u8],
        head_only: bool,
    ) -> std::io::Result<()> {
        // 204/304 MUST NOT carry Content-Length per RFC 9110 §8.6 (the
        // body was already suppressed by the caller for those).
        let mut out = if matches!(status, 204 | 304) {
            format!(
                "HTTP/1.1 {} {}\r\nConnection: close\r\n",
                status,
                reason(status)
            )
        } else {
            format!(
                "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                status,
                reason(status),
                content_type,
                body.len()
            )
        };
        for (k, v) in extra_headers {
            out.push_str(k);
            out.push_str(": ");
            out.push_str(v);
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
        stream.write_all(out.as_bytes())?;
        if !head_only {
            stream.write_all(body)?;
        }
        stream.flush()
    }

    pub fn write_error(stream: &mut std::net::TcpStream, status: u16, msg: &str) {
        let body = format!("{} {}\n{}\n", status, reason(status), msg);
        let _ = write_response(
            stream,
            status,
            "text/plain; charset=utf-8",
            &[],
            body.as_bytes(),
            false,
        );
    }

    pub fn mime_for(path: &std::path::Path) -> &'static str {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("html" | "htm") => "text/html; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("js" | "mjs") => "text/javascript; charset=utf-8",
            Some("json") => "application/json",
            Some("xml") => "application/xml",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            Some("ico") => "image/x-icon",
            Some("txt" | "md" | "mix" | "log" | "conf" | "toml" | "yaml" | "yml") => {
                "text/plain; charset=utf-8"
            }
            Some("pdf") => "application/pdf",
            Some("wasm") => "application/wasm",
            Some("mp4") => "video/mp4",
            Some("woff") => "font/woff",
            Some("woff2") => "font/woff2",
            _ => "application/octet-stream",
        }
    }

    pub fn html_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
}

/// Shared accept-with-deadline loop: non-blocking listener polled at
/// 15 ms, so `duration`/`timeout` deadlines and request counts are
/// honoured without a dedicated thread.
pub fn http_accept_next(
    listener: &std::net::TcpListener,
    deadline: Option<std::time::Instant>,
) -> MixResult<Option<(std::net::TcpStream, std::net::SocketAddr)>> {
    loop {
        // Ctrl-C ends a blocking server cleanly (the flag is set by the
        // evaluator's SIGINT handler). Without this the non-blocking
        // accept loop spins forever and ^C does nothing — a server you
        // cannot stop. `None` here means "loop over", so the caller
        // returns the count served so far, exactly like a duration
        // expiry, and the interpreter then raises the interrupt itself.
        if mix::interrupt::is_interrupted() {
            return Ok(None);
        }
        match listener.accept() {
            Ok((stream, peer)) => {
                stream.set_nonblocking(false).ok();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                    .ok();
                stream
                    .set_write_timeout(Some(std::time::Duration::from_secs(10)))
                    .ok();
                return Ok(Some((stream, peer)));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(d) = deadline
                    && std::time::Instant::now() >= d
                {
                    return Ok(None);
                }
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            // (accepted streams get a 1s read tick below, NOT their whole
            // budget: the per-request DEADLINE in read_head/read_body is
            // what bounds a slowloris, and it can only fire if reads
            // return control often enough to check the clock.)
            Err(e) => {
                return Err(MixError::RuntimeError {
                    span: None,
                    msg: format!("accept: {e}"),
                });
            }
        }
    }
}

/// `http_serve(root[, opts])` — blocking static file server; returns the
/// number of requests served once `duration`/`requests` ends it.
pub fn builtin_http_serve(args: Vec<Value>) -> MixResult<Option<Value>> {
    expect_args_between("http_serve", &args, 1, 2)?;
    let root_arg = args[0].to_mix_string();
    let root = std::fs::canonicalize(&root_arg).map_err(|e| MixError::RuntimeError {
        span: None,
        msg: format!("http_serve '{root_arg}': {e}"),
    })?;
    if !root.is_dir() {
        return Err(MixError::RuntimeError {
            span: None,
            msg: format!("http_serve: '{root_arg}' is not a directory"),
        });
    }

    let mut port: u16 = 0;
    let mut host = "127.0.0.1".to_string();
    let mut duration = 0.0f64;
    let mut index = "index.html".to_string();
    let mut listing = false;
    let mut render_md = false;
    let mut requests_cap: u64 = 0;
    let mut spa: Option<String> = None;
    let mut clean_urls = false;
    if let Some(opts) = args.get(1) {
        match opts {
            Value::Nil => {}
            Value::Map(m) => {
                for k in m.keys() {
                    if !matches!(
                        k.as_str(),
                        "port"
                            | "host"
                            | "duration"
                            | "index"
                            | "listing"
                            | "render_md"
                            | "requests"
                            | "spa"
                            | "clean_urls"
                    ) {
                        return Err(MixError::RuntimeError {
                            span: None,
                            msg: format!(
                                "http_serve(): unknown option '{k}' (supported: port, host, duration, index, listing, render_md, requests, spa, clean_urls)"
                            ),
                        });
                    }
                }
                if let Some(v) = m.get("port") {
                    port = as_exact_integer(
                        "http_serve(): option 'port'",
                        required_number_value("http_serve(): option 'port'", v)?,
                        0,
                        65535,
                    )? as u16;
                }
                if let Some(v) = m.get("host") {
                    host = v.to_mix_string();
                }
                if let Some(v) = m.get("duration") {
                    duration = required_number_value("http_serve(): option 'duration'", v)?;
                    as_duration("http_serve(): option 'duration'", duration)?;
                }
                if let Some(v) = m.get("index") {
                    index = v.to_mix_string();
                }
                if let Some(b) = bool_option("http_serve()", "listing", m.get("listing"))? {
                    listing = b;
                }
                if let Some(b) = bool_option("http_serve()", "render_md", m.get("render_md"))? {
                    render_md = b;
                }
                if let Some(v) = m.get("requests") {
                    requests_cap = as_exact_integer(
                        "http_serve(): option 'requests'",
                        required_number_value("http_serve(): option 'requests'", v)?,
                        0,
                        i64::MAX,
                    )? as u64;
                }
                if let Some(v) = m.get("spa") {
                    // The single-page-app fallback: a GET for a path with no
                    // matching file is answered with this shell file (200)
                    // instead of a 404, so a client-side router boots on every
                    // deep URL. `spa: true` means index.html; `spa: "404.html"`
                    // names another shell. Static frontend only — this serves
                    // the app, never its API. Resolved and prefix-checked once
                    // here so a bad shell path fails at startup, not per-404.
                    let shell_name = match v {
                        Value::Bool(true) => index.clone(),
                        // Every other option here treats nil/false as
                        // "off" — spa must too, rather than looking for a
                        // shell literally named "nil"/"0".
                        Value::Bool(false) | Value::Nil => String::new(),
                        Value::Number(_) => {
                            return Err(MixError::RuntimeError {
                                span: None,
                                msg: "http_serve(): option 'spa' must be true or a shell filename"
                                    .to_string(),
                            });
                        }
                        other => other.to_mix_string(),
                    };
                    if !shell_name.is_empty() {
                        let shell = root.join(&shell_name);
                        let canon =
                            std::fs::canonicalize(&shell).map_err(|e| MixError::RuntimeError {
                                span: None,
                                msg: format!(
                                    "http_serve(): spa shell '{shell_name}' not found under root: {e}"
                                ),
                            })?;
                        if !canon.starts_with(&root) || !canon.is_file() {
                            return Err(MixError::RuntimeError {
                                span: None,
                                msg: format!(
                                    "http_serve(): spa shell '{shell_name}' must be a file under the served root"
                                ),
                            });
                        }
                        spa = Some(canon.to_string_lossy().into_owned());
                    }
                }
                if let Some(b) = bool_option("http_serve()", "clean_urls", m.get("clean_urls"))? {
                    clean_urls = b;
                }
            }
            other => {
                return Err(MixError::RuntimeError {
                    span: None,
                    msg: format!(
                        "http_serve(): options must be a map or nil, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    }
    #[cfg(not(feature = "markdown"))]
    if render_md {
        return Err(MixError::RuntimeError {
            span: None,
            msg: "http_serve(): render_md requires the `markdown` feature".to_string(),
        });
    }

    let listener =
        std::net::TcpListener::bind((host.as_str(), port)).map_err(|e| MixError::RuntimeError {
            span: None,
            msg: format!("http_serve: bind '{host}:{port}': {e}"),
        })?;
    listener.set_nonblocking(true).ok();
    let bound = listener.local_addr().map_err(|e| MixError::RuntimeError {
        span: None,
        msg: format!("http_serve: {e}"),
    })?;
    // Self-discovery: the caller (and its operator) always learns where —
    // essential with port 0, harmless otherwise.
    // (an IPv6 literal host needs brackets in the printed URL)
    let url_host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.clone()
    };
    println!(
        "http_serve: http://{}:{}/ root={} (GET/HEAD only, Connection: close{})",
        url_host,
        bound.port(),
        root.display(),
        if duration > 0.0 {
            format!(", for {duration}s")
        } else {
            String::new()
        }
    );

    let deadline = (duration > 0.0)
        .then(|| std::time::Instant::now() + std::time::Duration::from_secs_f64(duration));
    let mut served: u64 = 0;
    while let Some((mut stream, _peer)) = http_accept_next(&listener, deadline)? {
        // Each request gets at most 10s of read time, clamped to the
        // serve deadline — one slow client cannot hold the loop past
        // `duration` by more than that grace (documented in http.md).
        let mut req_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        if let Some(d) = deadline {
            req_deadline = req_deadline.min(d + std::time::Duration::from_secs(10));
        }
        http_serve_one(
            &mut stream,
            &root,
            &index,
            listing,
            render_md,
            spa.as_deref(),
            clean_urls,
            req_deadline,
        );
        served += 1;
        if requests_cap > 0 && served >= requests_cap {
            break;
        }
    }
    Ok(Some(Value::Number(served as f64)))
}

#[allow(clippy::too_many_arguments)]
pub fn http_serve_one(
    stream: &mut std::net::TcpStream,
    root: &std::path::Path,
    index: &str,
    listing: bool,
    render_md: bool,
    spa: Option<&str>,
    clean_urls: bool,
    req_deadline: std::time::Instant,
) {
    // clean_urls (GitHub Pages parity): for an EXTENSIONLESS path with no
    // file of its own, serve `<path>.html` when it exists — through the
    // same canonicalize+prefix gate as any request path. Returns true if
    // it served. Shared by the not-found arm AND the directory branch (a
    // `bus.html` section page sits beside a `bus/` children dir — Pages
    // resolves `/bus` to that page when there is no `bus/index.html`).
    let try_clean_url = |stream: &mut std::net::TcpStream, rel: &str, head_only: bool| -> bool {
        if !clean_urls || rel.is_empty() {
            return false;
        }
        if rel.rsplit('/').next().unwrap_or("").contains('.') {
            return false;
        }
        if let Ok(html) = std::fs::canonicalize(root.join(format!("{rel}.html")))
            && html.starts_with(root)
            && html.is_file()
        {
            http_send_file(stream, &html, render_md, head_only);
            return true;
        }
        false
    };
    // The SPA fallback: a would-be 404 on a path with NO extension serves
    // the shell (200) so a client-side router boots. Only extensionless
    // paths fall back — a missing /app.js or /style.css must stay a real
    // 404, or a broken asset silently becomes the HTML shell and the app
    // fails in a far more confusing way. render_md is threaded through so
    // an .md shell renders consistently with the same file served as index.
    let spa_fallback = |stream: &mut std::net::TcpStream, path: &str, head_only: bool| -> bool {
        let Some(shell) = spa else { return false };
        let last = path.rsplit('/').next().unwrap_or("");
        if last.contains('.') {
            return false;
        }
        http_send_file(stream, std::path::Path::new(shell), render_md, head_only);
        true
    };
    let owned = stream;
    let mut reader = std::io::BufReader::new(&mut *owned);
    let req = match http_srv::read_head(&mut reader, req_deadline) {
        Ok(r) => r,
        Err((status, msg)) => {
            let stream = reader.into_inner();
            return http_srv::write_error(stream, status, msg);
        }
    };
    let stream: &mut std::net::TcpStream = reader.into_inner();
    // Method gate FIRST: a POST with a body gets its honest 405, not a
    // 413 about a body this server was never going to read (any unread
    // body dies with the Connection: close).
    let head_only = req.method == "HEAD";
    if req.method != "GET" && !head_only {
        let _ = http_srv::write_response(
            stream,
            405,
            "text/plain; charset=utf-8",
            &[("Allow".to_string(), "GET, HEAD".to_string())],
            b"405 Method Not Allowed\n",
            false,
        );
        return;
    }
    // Strip query, percent-decode (PATH rules — %XX only, `+` stays a
    // plus), refuse NUL. Decode BEFORE the traversal check so %2e%2e%2f
    // cannot sneak past it.
    let raw_path = req.target.split('?').next().unwrap_or("/");
    let decoded = http_srv::percent_decode_path(raw_path);
    if decoded.contains('\0') {
        return http_srv::write_error(stream, 400, "NUL in path");
    }
    let rel = decoded.trim_start_matches('/');
    let joined = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    // canonicalize resolves symlinks and ../ alike; the PREFIX CHECK on
    // the canonical result is the whole traversal defence — a symlink
    // inside the root pointing outside it lands here too.
    let canon = match std::fs::canonicalize(&joined) {
        Ok(c) => c,
        Err(_) => {
            // No file of this exact name: clean-URL (/foo → /foo.html)
            // first — a specific page beats the generic shell — then the
            // SPA fallback, else a real 404.
            if try_clean_url(stream, rel, head_only) {
                return;
            }
            if spa_fallback(stream, &decoded, head_only) {
                return;
            }
            return http_srv::write_error(stream, 404, "not found");
        }
    };
    if !canon.starts_with(root) {
        return http_srv::write_error(stream, 403, "outside the served root");
    }

    if canon.is_dir() {
        // The index path goes through the SAME canonicalize+prefix gate as
        // every request-derived path — an operator-supplied
        // `index: "../x"` must not become the one ungated read.
        let idx = canon.join(index);
        if let Ok(idx_canon) = std::fs::canonicalize(&idx)
            && idx_canon.starts_with(root)
            && idx_canon.is_file()
        {
            return http_send_file(stream, &idx_canon, render_md, head_only);
        }
        if !listing {
            // A directory with no index: a sibling `<dir>.html` (a section
            // page beside its children dir) resolves first — Pages parity —
            // then the SPA shell, before we refuse. `/bus/` (trailing slash)
            // is a dir request, not a `/bus` page request, so clean_urls is
            // skipped for it; only the extensionless no-slash form matches.
            if !decoded.ends_with('/') && try_clean_url(stream, rel, head_only) {
                return;
            }
            if spa_fallback(stream, &decoded, head_only) {
                return;
            }
            return http_srv::write_error(stream, 403, "directory listing disabled");
        }
        let mut names: Vec<(String, bool)> = match std::fs::read_dir(&canon) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| {
                    let is_dir = e.path().is_dir();
                    (e.file_name().to_string_lossy().into_owned(), is_dir)
                })
                .collect(),
            Err(_) => return http_srv::write_error(stream, 500, "unreadable directory"),
        };
        names.sort();
        let shown = if decoded.ends_with('/') || rel.is_empty() {
            decoded.clone()
        } else {
            format!("{decoded}/")
        };
        let mut html = format!(
            "<!doctype html><meta charset=\"utf-8\"><title>{0}</title><h1>{0}</h1><ul>",
            http_srv::html_escape(&shown)
        );
        if !rel.is_empty() {
            html.push_str("<li><a href=\"../\">../</a></li>");
        }
        for (name, is_dir) in names {
            let suffix = if is_dir { "/" } else { "" };
            // href gets percent-encoding (a `#` or space in a filename must
            // stay a working link); display text gets HTML escaping.
            let href = http_srv::href_encode(&name);
            let esc = http_srv::html_escape(&name);
            html.push_str(&format!(
                "<li><a href=\"{href}{suffix}\">{esc}{suffix}</a></li>"
            ));
        }
        html.push_str("</ul>");
        let _ = http_srv::write_response(
            stream,
            200,
            "text/html; charset=utf-8",
            &[],
            html.as_bytes(),
            head_only,
        );
        return;
    }
    http_send_file(stream, &canon, render_md, head_only)
}

pub fn http_send_file(
    stream: &mut std::net::TcpStream,
    path: &std::path::Path,
    render_md: bool,
    head_only: bool,
) {
    let is_md = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"));
    #[cfg(feature = "markdown")]
    if render_md && is_md {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return http_srv::write_error(stream, 500, "unreadable file"),
        };
        let rendered_val = api::markdown(vec![Value::String(src)]);
        let rendered = match &rendered_val {
            Ok(Some(Value::String(h))) => h.clone(),
            _ => return http_srv::write_error(stream, 500, "markdown render failed"),
        };
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let page = format!(
            "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{}</title><style>body{{max-width:52rem;margin:2rem auto;padding:0 1rem;font-family:system-ui,sans-serif;line-height:1.55}}pre{{background:#8881;padding:.7rem;overflow-x:auto}}code{{background:#8881;padding:.1em .25em}}pre code{{background:none;padding:0}}table{{border-collapse:collapse}}td,th{{border:1px solid #8884;padding:.25em .5em}}</style>{}",
            http_srv::html_escape(&title),
            rendered
        );
        let _ = http_srv::write_response(
            stream,
            200,
            "text/html; charset=utf-8",
            &[],
            page.as_bytes(),
            head_only,
        );
        return;
    }
    let _ = (render_md, is_md); // no-markdown builds: plain text below
    let body = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return http_srv::write_error(stream, 500, "unreadable file"),
    };
    let _ = http_srv::write_response(stream, 200, http_srv::mime_for(path), &[], &body, head_only);
}

/// `http_recv(port[, opts])` — accept ONE request, answer it, return it
/// as a map (nil on timeout). The OAuth-redirect / webhook-catch shape.
pub fn builtin_http_recv(args: Vec<Value>) -> MixResult<Option<Value>> {
    expect_args_between("http_recv", &args, 1, 2)?;
    let port = as_exact_integer(
        "http_recv(): port",
        number_arg("http_recv", &args, 0)?,
        1,
        65535,
    )? as u16;
    let mut timeout_seconds = 30.0;
    let mut host = "127.0.0.1".to_string();
    let mut max: usize = 1024 * 1024;
    let mut resp_status: u16 = 200;
    let mut resp_body: Vec<u8> = b"ok\n".to_vec();
    let mut resp_ct = "text/plain; charset=utf-8".to_string();
    let mut resp_headers: Vec<(String, String)> = Vec::new();
    if let Some(opts) = args.get(1) {
        match opts {
            Value::Nil => {}
            Value::Map(m) => {
                for k in m.keys() {
                    if !matches!(k.as_str(), "timeout" | "host" | "max" | "respond") {
                        return Err(MixError::RuntimeError {
                            span: None,
                            msg: format!(
                                "http_recv(): unknown option '{k}' (supported: timeout, host, max, respond)"
                            ),
                        });
                    }
                }
                if let Some(v) = m.get("timeout") {
                    timeout_seconds = required_number_value("http_recv(): option 'timeout'", v)?;
                    as_duration("http_recv(): option 'timeout'", timeout_seconds)?;
                }
                if let Some(v) = m.get("host") {
                    host = v.to_mix_string();
                }
                if let Some(v) = m.get("max") {
                    max = as_exact_integer(
                        "http_recv(): option 'max'",
                        required_number_value("http_recv(): option 'max'", v)?,
                        0,
                        64 * 1024 * 1024,
                    )? as usize;
                }
                if let Some(v) = m.get("respond") {
                    match v {
                        Value::Map(r) => {
                            for rk in r.keys() {
                                if !matches!(
                                    rk.as_str(),
                                    "status" | "body" | "content_type" | "headers"
                                ) {
                                    return Err(MixError::RuntimeError {
                                        span: None,
                                        msg: format!(
                                            "http_recv(): unknown respond option '{rk}' (supported: status, body, content_type, headers)"
                                        ),
                                    });
                                }
                            }
                            if let Some(s) = r.get("status") {
                                // 1xx would leave a compliant client waiting
                                // forever for the final response; refuse.
                                resp_status = as_exact_integer(
                                    "http_recv(): respond.status",
                                    required_number_value("http_recv(): respond.status", s)?,
                                    200,
                                    599,
                                )? as u16;
                            }
                            if let Some(b) = r.get("body") {
                                resp_body = match b {
                                    Value::Bytes(x) => x.to_vec(),
                                    Value::Buffer(x) => x.borrow().clone(),
                                    other => other.to_mix_string().into_bytes(),
                                };
                            }
                            if let Some(c) = r.get("content_type") {
                                resp_ct = c.to_mix_string();
                            }
                            if let Some(Value::Map(h)) = r.get("headers") {
                                for (hk, hv) in h.iter() {
                                    resp_headers.push((hk.clone(), hv.to_mix_string()));
                                }
                            }
                            // CR/LF in any respond value is response
                            // splitting — a script echoing an
                            // attacker-derived request field into a header
                            // must be refused loudly, never spliced. (This
                            // response is exactly what a browser renders in
                            // the OAuth-redirect flow.)
                            let mut hazard = resp_ct.contains(['\r', '\n']);
                            for (hk, hv) in &resp_headers {
                                if hk.contains(['\r', '\n', ':']) || hv.contains(['\r', '\n']) {
                                    hazard = true;
                                }
                            }
                            if hazard {
                                return Err(MixError::RuntimeError {
                                    span: None,
                                    msg: "http_recv(): respond header/content_type values must not contain CR, LF, or ':' in a header name — response splitting".to_string(),
                                });
                            }
                        }
                        other => {
                            return Err(MixError::RuntimeError {
                                span: None,
                                msg: format!(
                                    "http_recv(): option 'respond' must be a map, got {}",
                                    other.type_name()
                                ),
                            });
                        }
                    }
                }
            }
            other => {
                return Err(MixError::RuntimeError {
                    span: None,
                    msg: format!(
                        "http_recv(): options must be a map or nil, got {}",
                        other.type_name()
                    ),
                });
            }
        }
    }

    let listener =
        std::net::TcpListener::bind((host.as_str(), port)).map_err(|e| MixError::RuntimeError {
            span: None,
            msg: format!("http_recv: bind '{host}:{port}': {e}"),
        })?;
    listener.set_nonblocking(true).ok();
    let deadline = (timeout_seconds > 0.0)
        .then(|| std::time::Instant::now() + std::time::Duration::from_secs_f64(timeout_seconds));
    let Some((mut stream, peer)) = http_accept_next(&listener, deadline)? else {
        return Ok(Some(Value::Nil)); // timeout: an ordinary answer
    };
    let req_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut reader = std::io::BufReader::new(&mut stream);
    let bad = |stream: &mut std::net::TcpStream, status: u16, msg: &'static str| {
        http_srv::write_error(stream, status, msg);
        Err(MixError::RuntimeError {
            span: None,
            msg: format!("http_recv: bad request ({status}): {msg}"),
        })
    };
    let head = match http_srv::read_head(&mut reader, req_deadline) {
        Ok(h) => h,
        Err((status, msg)) => {
            drop(reader);
            return bad(&mut stream, status, msg);
        }
    };
    if head.content_length > max {
        drop(reader);
        return bad(&mut stream, 413, "body exceeds the configured max");
    }
    let body_bytes = match http_srv::read_body(&mut reader, head.content_length, req_deadline) {
        Ok(b) => b,
        Err((status, msg)) => {
            drop(reader);
            return bad(&mut stream, status, msg);
        }
    };
    drop(reader);
    let req = head;
    // 204/304 legally carry no body; suppress rather than lie about it.
    if matches!(resp_status, 204 | 304) {
        resp_body.clear();
    }
    let _ = http_srv::write_response(
        &mut stream,
        resp_status,
        &resp_ct,
        &resp_headers,
        &resp_body,
        req.method == "HEAD",
    );

    let (raw_path, query) = match req.target.split_once('?') {
        Some((p, q)) => (p, q.to_string()),
        None => (req.target.as_str(), String::new()),
    };
    let mut headers = indexmap::IndexMap::new();
    for (k, v) in &req.headers {
        headers.insert(k.clone(), Value::String(v.clone()));
    }
    let text = match std::str::from_utf8(&body_bytes) {
        Ok(s) => Value::String(s.to_string()),
        Err(_) => Value::Nil,
    };
    let mut out = indexmap::IndexMap::new();
    out.insert("method".to_string(), Value::String(req.method.clone()));
    out.insert(
        "path".to_string(),
        Value::String(http_srv::percent_decode_path(raw_path)),
    );
    out.insert("query".to_string(), Value::String(query));
    out.insert("headers".to_string(), Value::map(headers));
    out.insert("body".to_string(), text);
    out.insert("bytes".to_string(), Value::bytes(body_bytes));
    out.insert(
        "from_host".to_string(),
        Value::String(peer.ip().to_string()),
    );
    out.insert("from_port".to_string(), Value::Number(peer.port() as f64));
    Ok(Some(Value::map(out)))
}
