// SPDX-License-Identifier: MIT OR Apache-2.0
//! RFC 5804 ManageSieve. STARTTLS is the default transport; credentials and
//! account scripts are never accepted on the clear-text side of the upgrade.
use super::{EXTENSIONS, MAX_SCRIPT_BYTES, compile, store};
use crate::{
    config::Config,
    db::Db,
    tls::{ServerConfigCache, TlsSlot, current_tls_pair},
};
use anyhow::{Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpListener,
};

pub struct Handle {
    pub addrs: Vec<SocketAddr>,
}

pub async fn start(
    db: Db,
    cfg: &Config,
    slot: TlsSlot,
    cache: Arc<ServerConfigCache>,
) -> Result<Handle> {
    let addresses = cfg
        .sieve_listen
        .as_ref()
        .map(|s| s.to_vec())
        .unwrap_or_default();
    if addresses.is_empty() {
        return Ok(Handle { addrs: Vec::new() });
    }
    ensure!(
        current_tls_pair(&slot, &cache).is_some(),
        "ManageSieve requires a TLS identity"
    );
    let slots = Arc::new(tokio::sync::Semaphore::new(128));
    let mut addrs = Vec::new();
    // Bind all addresses before spawning so a failed bind leaves no listener.
    let mut listeners = Vec::new();
    for address in addresses {
        let listener = TcpListener::bind(&address).await?;
        addrs.push(listener.local_addr()?);
        listeners.push(listener);
    }
    for listener in listeners {
        let slot = slot.clone();
        let cache = cache.clone();
        let db = db.clone();
        let slots = slots.clone();
        let implicit = cfg.sieve_implicit_tls;
        tokio::spawn(async move {
            while let Ok((stream, peer)) = listener.accept().await {
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    continue;
                };
                let Some((_, config)) = current_tls_pair(&slot, &cache) else {
                    continue;
                };
                let db = db.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let run = async {
                        let stream = if implicit {
                            Some(stream)
                        } else {
                            session(stream, &db, false).await?
                        };
                        if let Some(stream) = stream {
                            let tls = tokio::time::timeout(
                                Duration::from_secs(30),
                                tokio_rustls::TlsAcceptor::from(config).accept(stream),
                            )
                            .await??;
                            session(tls, &db, true).await?;
                        }
                        Ok::<(), anyhow::Error>(())
                    }
                    .await;
                    if let Err(error) = run {
                        tracing::debug!(%peer,%error,"ManageSieve disconnected");
                    }
                });
            }
        });
    }
    Ok(Handle { addrs })
}

fn quoted(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .chars()
            .filter(|c| !c.is_control())
            .collect::<String>()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}
async fn write<S: AsyncRead + AsyncWrite + Unpin>(
    io: &mut BufReader<S>,
    response: &str,
) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(30), async {
        io.get_mut().write_all(response.as_bytes()).await?;
        io.get_mut().flush().await
    })
    .await??;
    Ok(())
}
fn capabilities(secure: bool) -> String {
    format!(
        "\"IMPLEMENTATION\" \"MixOS maild\"\r\n\"VERSION\" \"1.0\"\r\n\"SIEVE\" \"{EXTENSIONS}\"\r\n\"SASL\" \"{}\"\r\n{}OK \"ManageSieve ready\"\r\n",
        if secure { "PLAIN LOGIN" } else { "" },
        if secure { "" } else { "\"STARTTLS\"\r\n" }
    )
}

async fn line<S: AsyncRead + Unpin>(io: &mut BufReader<S>) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        let b = match io.read_u8().await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && bytes.is_empty() => {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        bytes.push(b);
        ensure!(bytes.len() <= 8192, "command line limit");
        if b == b'\n' {
            ensure!(bytes.ends_with(b"\r\n"), "CRLF required");
            bytes.truncate(bytes.len() - 2);
            return Ok(Some(String::from_utf8(bytes)?));
        }
    }
}

/// Parse strings and non-synchronising octet literals, including embedded CRLF.
/// Limit the entire command, not just individual literals. No unbounded read_line.
async fn command<S: AsyncRead + Unpin>(io: &mut BufReader<S>) -> Result<Option<Vec<String>>> {
    let Some(mut input) = line(io).await? else {
        return Ok(None);
    };
    let mut result = Vec::new();
    let mut total = input.len();
    let mut p = 0;
    loop {
        let bytes = input.as_bytes();
        while bytes.get(p) == Some(&b' ') || bytes.get(p) == Some(&b'\t') {
            p += 1;
        }
        if p == bytes.len() {
            return Ok(Some(result));
        }
        ensure!(result.len() < 8, "too many arguments");
        if bytes[p] == b'"' {
            p += 1;
            let mut out = Vec::new();
            let mut closed = false;
            while p < bytes.len() {
                let b = bytes[p];
                p += 1;
                if b == b'"' {
                    closed = true;
                    break;
                }
                if b == b'\\' {
                    let next = *bytes
                        .get(p)
                        .ok_or_else(|| anyhow::anyhow!("invalid quoted escape"))?;
                    ensure!(next == b'"' || next == b'\\', "invalid quoted escape");
                    out.push(next);
                    p += 1;
                } else {
                    ensure!(!b.is_ascii_control(), "control in quoted string");
                    out.push(b);
                }
            }
            ensure!(closed, "unterminated quoted string");
            result.push(String::from_utf8(out)?);
        } else if bytes[p] == b'{' {
            let n = input[p..]
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix("+}"))
                .ok_or_else(|| anyhow::anyhow!("expected non-synchronising literal"))?
                .parse::<usize>()?;
            total = total
                .checked_add(n)
                .ok_or_else(|| anyhow::anyhow!("literal overflow"))?;
            ensure!(total <= MAX_SCRIPT_BYTES + 8192, "literal quota exceeded");
            let mut literal = vec![0; n];
            io.read_exact(&mut literal).await?;
            result.push(String::from_utf8(literal)?);
            input = line(io)
                .await?
                .ok_or_else(|| anyhow::anyhow!("missing command terminator"))?;
            total += input.len();
            ensure!(total <= MAX_SCRIPT_BYTES + 8192, "command quota exceeded");
            p = 0;
        } else {
            let start = p;
            while p < bytes.len() && bytes[p] != b' ' && bytes[p] != b'\t' {
                ensure!(bytes[p].is_ascii_graphic(), "invalid atom");
                p += 1;
            }
            result.push(input[start..p].to_string());
        }
        ensure!(
            p == input.len() || input.as_bytes()[p].is_ascii_whitespace(),
            "missing argument separator"
        );
    }
}

async fn authentication<S: AsyncRead + AsyncWrite + Unpin>(
    io: &mut BufReader<S>,
    db: &Db,
    args: &[String],
) -> Result<Option<i32>> {
    ensure!(
        !args.is_empty() && args.len() <= 2,
        "AUTHENTICATE mechanism [initial-response]"
    );
    let mechanism = args[0].to_ascii_uppercase();
    let initial = if let Some(value) = args.get(1) {
        value.clone()
    } else {
        write(
            io,
            if mechanism == "LOGIN" {
                "\"VXNlcm5hbWU6\"\r\n"
            } else {
                "\"\"\r\n"
            },
        )
        .await?;
        let response = command(io)
            .await?
            .ok_or_else(|| anyhow::anyhow!("authentication EOF"))?;
        ensure!(response.len() == 1, "invalid SASL response");
        response[0].clone()
    };
    if initial == "*" {
        bail!("authentication cancelled");
    }
    let decoded = String::from_utf8(STANDARD.decode(initial)?)?;
    let (user, password) = if mechanism == "PLAIN" {
        let parts: Vec<&str> = decoded.split('\0').collect();
        ensure!(
            parts.len() == 3 && (parts[0].is_empty() || parts[0] == parts[1]),
            "SASL authorisation identity refused"
        );
        (parts[1].to_string(), parts[2].to_string())
    } else if mechanism == "LOGIN" {
        write(io, "\"UGFzc3dvcmQ6\"\r\n").await?;
        let response = command(io)
            .await?
            .ok_or_else(|| anyhow::anyhow!("authentication EOF"))?;
        ensure!(response.len() == 1, "invalid SASL response");
        (decoded, String::from_utf8(STANDARD.decode(&response[0])?)?)
    } else {
        bail!("unsupported SASL mechanism");
    };
    crate::auth::basic::verify(db, &user, &password).await
}

/// A clear-text session returns its transport only on STARTTLS. All parser
/// buffers are discarded at that boundary, as RFC 5804 requires. Tests use the
/// same function over duplex streams for already-established TLS sessions.
pub async fn session<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    db: &Db,
    secure: bool,
) -> Result<Option<S>> {
    let mut io = BufReader::new(stream);
    let mut account = None;
    let mut failures = 0;
    write(&mut io, &capabilities(secure)).await?;
    loop {
        let idle = if account.is_some() { 1800 } else { 180 };
        let command = tokio::time::timeout(Duration::from_secs(idle), command(&mut io)).await;
        let args = match command {
            Ok(Ok(Some(args))) if !args.is_empty() => args,
            Ok(Ok(None)) => return Ok(None),
            _ => {
                write(
                    &mut io,
                    "BYE \"Invalid or oversized command, or timeout\"\r\n",
                )
                .await?;
                return Ok(None);
            }
        };
        let verb = args[0].to_ascii_uppercase();
        let args = &args[1..];
        if verb == "LOGOUT" && args.is_empty() {
            write(&mut io, "OK \"Logout\"\r\n").await?;
            return Ok(None);
        }
        if verb == "CAPABILITY" && args.is_empty() {
            write(&mut io, &capabilities(secure)).await?;
            continue;
        }
        if verb == "NOOP" && args.len() <= 1 {
            write(
                &mut io,
                &args
                    .first()
                    .map(|s| format!("OK (TAG {}) \"Done\"\r\n", quoted(s)))
                    .unwrap_or_else(|| "OK \"Done\"\r\n".into()),
            )
            .await?;
            continue;
        }
        if verb == "STARTTLS" {
            if !secure && account.is_none() && args.is_empty() {
                write(&mut io, "OK \"Begin TLS negotiation\"\r\n").await?;
                return Ok(Some(io.into_inner()));
            }
            write(&mut io, "NO \"STARTTLS unavailable\"\r\n").await?;
            continue;
        }
        if !secure {
            write(&mut io, "NO (ENCRYPT-NEEDED) \"Use STARTTLS\"\r\n").await?;
            continue;
        }
        if verb == "AUTHENTICATE" {
            if account.is_some() {
                write(&mut io, "NO \"Already authenticated\"\r\n").await?;
                continue;
            }
            let auth =
                tokio::time::timeout(Duration::from_secs(30), authentication(&mut io, db, args))
                    .await;
            match auth {
                Ok(Ok(Some(id))) => {
                    account = Some(id);
                    write(&mut io, "OK \"Authenticated\"\r\n").await?;
                }
                _ => {
                    failures += 1;
                    write(&mut io, "NO \"Authentication failed\"\r\n").await?;
                    if failures >= 3 {
                        write(&mut io, "BYE \"Too many failures\"\r\n").await?;
                        return Ok(None);
                    }
                }
            }
            continue;
        }
        let Some(id) = account else {
            write(&mut io, "NO \"Authenticate first\"\r\n").await?;
            continue;
        };
        let response = execute(db, id, &verb, args).await;
        match response {
            Ok(response) => write(&mut io, &response).await?,
            Err(error) => {
                let message = error.to_string();
                let code = [
                    "NONEXISTENT",
                    "ALREADYEXISTS",
                    "ACTIVE",
                    "QUOTA/MAXSIZE",
                    "QUOTA/MAXSCRIPTS",
                ]
                .into_iter()
                .find(|code| message.starts_with(code))
                .map(|c| format!(" ({c})"))
                .unwrap_or_default();
                write(&mut io, &format!("NO{code} {}\r\n", quoted(&message))).await?;
            }
        }
    }
}

async fn execute(db: &Db, account: i32, verb: &str, args: &[String]) -> Result<String> {
    let arity = match verb {
        "LISTSCRIPTS" => 0,
        "GETSCRIPT" | "CHECKSCRIPT" | "SETACTIVE" | "DELETESCRIPT" => 1,
        "PUTSCRIPT" | "RENAMESCRIPT" | "HAVESPACE" => 2,
        _ => bail!("unknown command"),
    };
    ensure!(args.len() == arity, "wrong argument count");
    let mut response = String::new();
    match verb {
        "LISTSCRIPTS" => {
            for entry in store::list(db, account).await? {
                response.push_str(&format!(
                    "{}{}\r\n",
                    quoted(&entry.name),
                    if entry.active { " ACTIVE" } else { "" }
                ));
            }
        }
        "GETSCRIPT" => {
            let script = store::get(db, account, &args[0]).await?;
            response = format!("{{{}}}\r\n{}\r\n", script.source.len(), script.source);
        }
        "PUTSCRIPT" => {
            store::put(db, account, &args[0], &args[1], None).await?;
        }
        "CHECKSCRIPT" => {
            let source = args[0].clone();
            tokio::task::spawn_blocking(move || compile(&source)).await??;
        }
        "SETACTIVE" => store::activate(db, account, &args[0]).await?,
        "DELETESCRIPT" => store::delete(db, account, &args[0]).await?,
        "RENAMESCRIPT" => store::rename(db, account, &args[0], &args[1]).await?,
        "HAVESPACE" => store::have_space(db, account, &args[0], args[1].parse()?).await?,
        _ => unreachable!(),
    }
    response.push_str("OK \"Done\"\r\n");
    Ok(response)
}
