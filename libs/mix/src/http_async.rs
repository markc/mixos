// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Scoped HTTP response reads. Dropping the future drops the response/socket;
//! no reader thread or handle can outlive the evaluator that called it.
use super::*;
use crate::evaluator::Evaluator;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

type Reply<'a> = Pin<Box<dyn Future<Output = MixResult<Value>> + 'a>>;

pub(crate) fn get(eval: &mut Evaluator, args: Vec<Value>) -> Reply<'_> {
    Box::pin(buffered(eval, "http_get", args))
}

pub(crate) fn post(eval: &mut Evaluator, args: Vec<Value>) -> Reply<'_> {
    Box::pin(buffered(eval, "http_post", args))
}

pub(crate) fn request(eval: &mut Evaluator, args: Vec<Value>) -> Reply<'_> {
    Box::pin(buffered(eval, "http_request", args))
}

async fn buffered(eval: &mut Evaluator, name: &str, args: Vec<Value>) -> MixResult<Value> {
    // Resolve slots before parsing options: ordinary calls must not load a CA
    // twice (or otherwise alter the established path's observable behaviour).
    if !conditional_cap_engaged(name, &args, "idle_timeout") {
        return Ok(call_builtin(name, args)?.unwrap_or(Value::Nil));
    }
    let (method, url, body, headers, opts) = match name {
        "http_get" => {
            expect_args_between(name, &args, 1, 3)?;
            let (headers, opts) = http_headers_and_timeout(name, args.get(1), args.get(2))?;
            ("GET".into(), args[0].to_mix_string(), None, headers, opts)
        }
        "http_post" => {
            expect_args_between(name, &args, 2, 4)?;
            let (headers, opts) = http_headers_and_timeout(name, args.get(2), args.get(3))?;
            (
                "POST".into(),
                args[0].to_mix_string(),
                Some(args[1].clone()),
                headers,
                opts,
            )
        }
        _ => {
            expect_args_between(name, &args, 2, 5)?;
            let short_opts = args.len() == 3
                && matches!(args.get(2), Some(Value::Map(m)) if http_map_is_opts(m));
            let (headers, opts) = if short_opts {
                (Vec::new(), parse_http_opts(name, args.get(2))?)
            } else {
                http_headers_and_timeout(name, args.get(3), args.get(4))?
            };
            (
                args[0].to_mix_string().to_uppercase(),
                args[1].to_mix_string(),
                if short_opts {
                    None
                } else {
                    args.get(2).cloned()
                },
                headers,
                opts,
            )
        }
    };
    perform(eval, &method, &url, body.as_ref(), &headers, opts, None).await
}

pub(crate) fn stream(eval: &mut Evaluator, args: Vec<Value>) -> Reply<'_> {
    Box::pin(async move {
        expect_args_between("http_stream", &args, 3, 6)?;
        let callback = match &args[2] {
            Value::Function(f) => f.clone(),
            _ => {
                return Err(MixError::structured(
                    "TYPE_MISMATCH",
                    "http_stream: on_line must be a function",
                ));
            }
        };
        let (headers, mut opts) =
            http_headers_and_timeout("http_stream", args.get(4), args.get(5))?;
        opts.idle.get_or_insert(Duration::from_secs(30));
        perform(
            eval,
            &args[0].to_mix_string().to_uppercase(),
            &args[1].to_mix_string(),
            args.get(3),
            &headers,
            opts,
            Some(&callback),
        )
        .await
    })
}

fn failure(map: &mut indexmap::IndexMap<String, Value>, code: &str, error: impl std::fmt::Display) {
    map.insert("status".into(), Value::Number(0.0));
    map.insert("error_code".into(), Value::String(code.into()));
    map.insert("error".into(), Value::String(error.to_string()));
}

/// Cancellation is sampled independently of the HTTP read, including while
/// waiting for headers. No blocking syscall occupies the evaluator thread.
async fn interrupted(flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    loop {
        if flag.load(std::sync::atomic::Ordering::Relaxed) || crate::interrupt::is_interrupted() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// Only cancel network futures. Dropping a callback future midway through its
// evaluator frame would skip the normal scope/depth restoration. Callbacks
// unwind through the evaluator's own interrupt checkpoints instead.
async fn io_wait<T>(
    operation: impl Future<Output = T>,
    flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    deadline: Option<Instant>,
) -> Result<T, (&'static str, &'static str)> {
    if let Some(reason) = stop_reason(flag, deadline) {
        return Err(reason);
    }
    let expired = async {
        if let Some(deadline) = deadline {
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        biased;
        () = interrupted(flag.clone()) => Err(("HTTP_INTERRUPTED", "HTTP request interrupted")),
        () = expired => Err(("HTTP_TIMEOUT", "HTTP whole-call timeout expired")),
        output = operation => Ok(output),
    }
}

fn stop_reason(
    flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    deadline: Option<Instant>,
) -> Option<(&'static str, &'static str)> {
    if flag.load(std::sync::atomic::Ordering::Relaxed) || crate::interrupt::is_interrupted() {
        Some(("HTTP_INTERRUPTED", "HTTP request interrupted"))
    } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        Some(("HTTP_TIMEOUT", "HTTP whole-call timeout expired"))
    } else {
        None
    }
}

fn whole_deadline(started: Instant, timeout_s: u64) -> Option<Instant> {
    if timeout_s == 0 {
        None
    } else {
        started.checked_add(Duration::from_secs(timeout_s))
    }
}

async fn perform(
    eval: &mut Evaluator,
    method: &str,
    url: &str,
    body: Option<&Value>,
    headers: &[(String, String)],
    opts: HttpOpts,
    callback: Option<&crate::scope::MixFunction>,
) -> MixResult<Value> {
    let started = Instant::now();
    let deadline = whole_deadline(started, opts.timeout_s);
    let mut map = indexmap::IndexMap::from([
        ("status".into(), Value::Number(0.0)),
        ("headers".into(), Value::map(indexmap::IndexMap::new())),
        ("final_url".into(), Value::String(url.into())),
        ("error_code".into(), Value::Nil),
        ("error".into(), Value::Nil),
    ]);
    if callback.is_some() {
        map.insert("stopped".into(), Value::Bool(false));
    }
    let mut bytes = Vec::new();
    let mut received = 0usize;
    let mut pending = Vec::new();
    let flag = eval.interrupt_flag();
    let result = async {
        if !is_http_token(method) {
            failure(&mut map, "HTTP_PROTOCOL", "invalid HTTP method");
            return Ok::<(), MixError>(());
        }
        let mut builder = reqwest::Client::builder()
            .http1_only()
            .no_brotli()
            // Implement the existing ureq redirect contract below, including
            // its refusal to replay POST bodies on 307/308.
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(0);
        if let Some(idle) = opts.idle {
            builder = builder.read_timeout(idle).connect_timeout(idle);
        }
        // Use the very same root bundle and verifier as the ureq builtins,
        // rather than reqwest's independently versioned webpki-roots bundle.
        let tls = if opts.insecure {
            insecure_http_config()
        } else if let Some(tls) = opts.ca_agent {
            tls
        } else {
            let mut roots = ureq::rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            std::sync::Arc::new(
                ureq::rustls::ClientConfig::builder_with_provider(
                    ureq::rustls::crypto::ring::default_provider().into(),
                )
                .with_safe_default_protocol_versions()
                .expect("ring supports default TLS versions")
                .with_root_certificates(roots)
                .with_no_client_auth(),
            )
        };
        builder = builder.use_preconfigured_tls((*tls).clone());
        let client = match builder.build() {
            Ok(client) => client,
            Err(e) => {
                failure(&mut map, "HTTP_TLS", e);
                return Ok(());
            }
        };
        let mut current_url = match reqwest::Url::parse(url) {
            Ok(url) => url,
            Err(e) => {
                failure(&mut map, "HTTP_PROTOCOL", e);
                return Ok(());
            }
        };
        let mut current_method = method.to_string();
        let mut current_body = body.filter(|v| !matches!(v, Value::Nil));
        let mut current_headers = headers.to_vec();
        let mut redirects = 0;
        let mut response = loop {
            // ureq encodes URL userinfo literally, without percent-decoding,
            // and an explicit Authorization header takes precedence. Prevent
            // reqwest from applying its different automatic userinfo handling.
            let mut request_url = current_url.clone();
            let userinfo = !current_url.username().is_empty()
                || current_url
                    .password()
                    .is_some_and(|password| !password.is_empty());
            if request_url.has_host() {
                let _ = request_url.set_username("");
                let _ = request_url.set_password(None);
            }
            let mut request = client.request(
                reqwest::Method::from_bytes(current_method.as_bytes()).expect("validated token"),
                request_url,
            );
            if userinfo
                && !current_headers
                    .iter()
                    .any(|(key, _)| key.eq_ignore_ascii_case("authorization"))
            {
                request = request.basic_auth(
                    current_url.username(),
                    Some(current_url.password().unwrap_or("")),
                );
            }
            for (key, value) in &current_headers {
                request = request.header(key.as_str(), value.as_str());
            }
            if let Some(value) = current_body {
                let (text, raw) = http_body_from(value);
                request = if let Some(raw) = raw {
                    request.body(raw)
                } else {
                    let request = request.body(text.unwrap_or_default());
                    if current_headers
                        .iter()
                        .any(|(key, _)| key.eq_ignore_ascii_case("content-type"))
                    {
                        request
                    } else {
                        request.header(reqwest::header::CONTENT_TYPE, "text/plain; charset=utf-8")
                    }
                };
            }
            let response = match io_wait(request.send(), &flag, deadline).await {
                Ok(Ok(response)) => response,
                Err((code, error)) => {
                    failure(&mut map, code, error);
                    return Ok(());
                }
                Ok(Err(e)) => {
                    let code = if e.is_timeout() {
                        "HTTP_IDLE_TIMEOUT"
                    } else if source_is_tls(&e) {
                        "HTTP_TLS"
                    } else if e.is_connect() {
                        "HTTP_CONNECT"
                    } else {
                        "HTTP_PROTOCOL"
                    };
                    failure(&mut map, code, e);
                    return Ok(());
                }
            };
            let status = response.status().as_u16();
            if !(300..399).contains(&status) {
                break response;
            }
            // ureq's configured budget is 5 requests: the fifth 3xx fails,
            // before even consulting Location. Preserve that boundary.
            if redirects + 1 >= 5 {
                failure(&mut map, "HTTP_REDIRECT", "reached max redirects (5)");
                return Ok(());
            }
            let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
                break response;
            };
            let next_url = match location
                .to_str()
                .ok()
                .and_then(|s| current_url.join(s).ok())
            {
                Some(url) => url,
                None => {
                    failure(&mut map, "HTTP_PROTOCOL", "invalid redirect URL");
                    return Ok(());
                }
            };
            match status {
                301..=303 => {
                    if !["GET", "HEAD"].contains(&current_method.as_str()) {
                        current_method = "GET".into();
                    }
                }
                307 | 308
                    if ["GET", "HEAD", "OPTIONS", "TRACE"].contains(&current_method.as_str()) => {}
                _ => break response,
            }
            current_headers.retain(|(key, _)| {
                !["host", "content-length", "cookie", "authorization"]
                    .contains(&key.to_ascii_lowercase().as_str())
            });
            current_body = None;
            current_url = next_url;
            redirects += 1;
        };
        let status = response.status().as_u16();
        map.insert("status".into(), Value::Number(status as f64));
        map.insert("final_url".into(), Value::String(current_url.to_string()));
        let mut response_headers = indexmap::IndexMap::new();
        for key in response.headers().keys() {
            response_headers.insert(
                key.as_str().into(),
                Value::list(
                    response
                        .headers()
                        .get_all(key)
                        .iter()
                        .map(|v| Value::String(String::from_utf8_lossy(v.as_bytes()).into_owned()))
                        .collect(),
                ),
            );
        }
        map.insert("headers".into(), Value::map(response_headers));
        if response_is_bodyless(method, status) {
            return Ok(());
        }
        loop {
            let chunk = match io_wait(response.chunk(), &flag, deadline).await {
                Ok(Ok(chunk)) => chunk,
                Err((code, error)) => {
                    failure(&mut map, code, error);
                    return Ok(());
                }
                Ok(Err(e)) => {
                    let code = if e.is_timeout() {
                        "HTTP_IDLE_TIMEOUT"
                    } else {
                        "HTTP_STREAM_CUT"
                    };
                    failure(&mut map, code, e);
                    return Ok(());
                }
            };
            let Some(chunk) = chunk else {
                if let Some(callback) = callback
                    && !pending.is_empty()
                {
                    if std::str::from_utf8(&pending).is_err() {
                        failure(&mut map, "HTTP_TEXT", "http_stream: line is not UTF-8");
                        return Ok(());
                    }
                    let keep_reading = deliver(eval, callback, &pending).await?;
                    pending.clear();
                    if let Some((code, error)) = stop_reason(&flag, deadline) {
                        failure(&mut map, code, error);
                        return Ok(());
                    }
                    if !keep_reading {
                        map.insert("stopped".into(), Value::Bool(true));
                    }
                }
                return Ok(());
            };
            received += chunk.len();
            // Buffered responses keep the original cap. Streams retain only
            // the current line (also capped), never the complete response.
            if let Some(callback) = callback {
                for &byte in &chunk {
                    pending.push(byte);
                    if pending.len() as u64 > MAX_HTTP_BODY_BYTES {
                        failure(&mut map, "HTTP_BODY_LIMIT", "line exceeds 64 MiB cap");
                        return Ok(());
                    }
                    if byte == b'\n' {
                        if std::str::from_utf8(&pending).is_err() {
                            failure(&mut map, "HTTP_TEXT", "http_stream: line is not UTF-8");
                            return Ok(());
                        }
                        let keep_reading = deliver(eval, callback, &pending).await?;
                        pending.clear();
                        if let Some((code, error)) = stop_reason(&flag, deadline) {
                            failure(&mut map, code, error);
                            return Ok(());
                        }
                        if !keep_reading {
                            map.insert("stopped".into(), Value::Bool(true));
                            return Ok(());
                        }
                    }
                }
            } else {
                if bytes.len() as u64 + chunk.len() as u64 > MAX_HTTP_BODY_BYTES {
                    failure(&mut map, "HTTP_BODY_LIMIT", "response exceeds 64 MiB cap");
                    return Ok(());
                }
                bytes.extend_from_slice(&chunk);
            }
        }
    }
    .await;
    result?;
    if callback.is_some() {
        // Complete lines have already been delivered; preserve the unfinished
        // line on failure for diagnostics/recovery, without duplicating them.
        bytes = pending;
    }
    let status = match map.get("status") {
        Some(Value::Number(n)) => *n as u16,
        _ => 0,
    };
    http_body_into_map(status, bytes, &mut map);
    if callback.is_some() || status == 0 {
        map.insert("bytes_received".into(), Value::Number(received as f64));
    }
    map.insert(
        "duration_ms".into(),
        Value::Number(started.elapsed().as_secs_f64() * 1000.0),
    );
    Ok(Value::map(map))
}

async fn deliver(
    eval: &mut Evaluator,
    callback: &crate::scope::MixFunction,
    raw: &[u8],
) -> MixResult<bool> {
    let line = if let Some(line) = raw.strip_suffix(b"\n") {
        line.strip_suffix(b"\r").unwrap_or(line)
    } else {
        raw
    };
    let text = std::str::from_utf8(line)
        .map_err(|_| MixError::structured("HTTP_TEXT", "http_stream: line is not UTF-8"))?;
    let result = eval
        .call_function_public(callback, &[Value::String(text.into())])
        .await?;
    Ok(!matches!(result, Value::Bool(false)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_classification_inspects_io_error_payloads() {
        let error = std::io::Error::other(ureq::rustls::Error::InvalidCertificate(
            ureq::rustls::CertificateError::UnknownIssuer,
        ));
        assert!(source_is_tls(&error));
        assert!(!source_is_tls(&std::io::Error::other("not a TLS error")));
    }

    #[test]
    fn timeout_zero_disables_only_the_whole_call_deadline() {
        let started = Instant::now();
        let opts = parse_http_opts(
            "http_get",
            Some(&Value::map(indexmap::IndexMap::from([
                ("timeout".into(), Value::Number(0.0)),
                ("idle_timeout".into(), Value::Number(0.1)),
            ]))),
        )
        .unwrap();
        assert_eq!(whole_deadline(started, opts.timeout_s), None);
        assert_eq!(opts.idle, Some(Duration::from_millis(100)));
        assert_eq!(
            whole_deadline(started, 1),
            Some(started + Duration::from_secs(1))
        );
    }
}
