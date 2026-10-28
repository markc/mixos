// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! IMAP4rev2 conformance wire tests — FETCH / UID FETCH metadata atoms.
//!
//! Uses the same in-process `handle_connection` harness as Phase 2
//! (tokio duplex; no TLS). Each test provisions a fresh sqlite Db +
//! MailStore + Mds, inserts known messages into INBOX, then drives
//! the wire to assert FETCH responses match RFC 9051 §6.4.5 / §6.4.8
//! shape for the metadata atoms (FLAGS, UID, RFC822.SIZE,
//! INTERNALDATE).

use std::sync::Arc;

use anyhow::Result;
use maild::db::Db;
use maild::imap::config::ImapConfig;
use maild::imap::session::{AccountSlots, handle_connection};
use maild::mailstore::{EmailEnvelope, MailStore, MailboxRole, SqliteMailStore};
use mds::types::ContainerAttrs;
use mds::{Flags, Mds, SqliteCasMds, Tags};
use rusqlite::params;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

fn attrs_for(role: Option<&str>, subscribed: bool) -> ContainerAttrs {
    ContainerAttrs {
        special_use: role.map(|s| s.to_string()),
        subscribed,
        extra: serde_json::Value::Null,
    }
}

async fn make_fixture(
    email: &str,
    password: &str,
) -> Result<(TempDir, Db, Arc<SqliteCasMds>, Arc<dyn MailStore>)> {
    let tmp = tempfile::tempdir()?;
    let dbp = tmp.path().join("mail.db").to_string_lossy().into_owned();
    let blob = tmp.path().join("blobs").to_string_lossy().into_owned();
    let db = Db::connect(&dbp, &blob).await?;
    db.migrate().await?;
    let hash = bcrypt::hash(password, 4)?;
    {
        let conn = db.conn.lock().expect("db lock");
        conn.execute(
            "INSERT INTO accounts (email, password, name) VALUES (?1, ?2, ?3)",
            params![email, hash, "Test"],
        )?;
    }
    let mds_dir = tmp.path().join("mds");
    let mds = Arc::new(SqliteCasMds::open(mds_dir)?);
    let mailstore: Arc<dyn MailStore> = Arc::new(SqliteMailStore::new(Arc::clone(&mds)));
    mailstore.create_mailbox(1, "INBOX", None, attrs_for(Some("\\Inbox"), true))?;
    mailstore.create_mailbox(1, "Archive", None, attrs_for(Some("\\Archive"), true))?;
    Ok((tmp, db, mds, mailstore))
}

async fn spawn_session(
    db: Db,
    mailstore: Arc<dyn MailStore>,
    cfg: ImapConfig,
) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<Result<()>>) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let slots = AccountSlots::new();
    let cfg = Arc::new(cfg);
    let peer = "127.0.0.1:0".parse().unwrap();
    let join = tokio::spawn(async move {
        handle_connection(
            server,
            peer,
            None,
            cfg,
            db,
            mailstore,
            slots,
            "localhost".to_string(),
        )
        .await
    });
    (client, join)
}

async fn read_line<R: tokio::io::AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Result<String> {
    let mut s = String::new();
    let n = reader.read_line(&mut s).await?;
    if n == 0 {
        return Err(anyhow::anyhow!("peer closed"));
    }
    Ok(s.trim_end_matches(['\r', '\n']).to_string())
}

async fn login(
    client: tokio::io::DuplexStream,
) -> Result<(
    BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    tokio::io::WriteHalf<tokio::io::DuplexStream>,
)> {
    let (r, mut w) = tokio::io::split(client);
    let mut reader = BufReader::new(r);
    let _greeting = read_line(&mut reader).await?;
    w.write_all(b"a01 LOGIN alice@example.com hunter2\r\n")
        .await?;
    let resp = read_line(&mut reader).await?;
    assert!(resp.starts_with("a01 OK"), "LOGIN failed: {resp}");
    Ok((reader, w))
}

async fn read_until_tagged<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    tag: &str,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    loop {
        let line = read_line(reader).await?;
        let is_tagged = line.starts_with(&format!("{tag} "));
        out.push(line);
        if is_tagged {
            return Ok(out);
        }
    }
}

fn insert_email(
    ms: &Arc<dyn MailStore>,
    mds: &Arc<SqliteCasMds>,
    body: &[u8],
    subject: &str,
    flags: Flags,
    received_at_ms: i64,
) -> Result<u32> {
    let inbox = ms
        .mailbox_by_role(1, MailboxRole::Inbox)?
        .expect("INBOX missing");
    let hash = mds.put_blob(body)?;
    let envelope = EmailEnvelope {
        from: "ext@example.com".into(),
        to: vec!["alice@example.com".into()],
        cc: Vec::new(),
        bcc: Vec::new(),
        reply_to: Vec::new(),
        subject: subject.into(),
        date: received_at_ms / 1000,
        message_id: Some(format!("<{subject}@example.com>")),
    };
    let (_item, ins) = ms.create_email(
        1,
        inbox,
        hash,
        envelope,
        &[],
        flags,
        Tags::new(),
        received_at_ms,
    )?;
    Ok(ins.uid as u32)
}

type Reader = BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>;
type Writer = tokio::io::WriteHalf<tokio::io::DuplexStream>;

/// Parse the wire fixture independently of the server's MIME parser and
/// section selector, then compare reported part types and sizes with FETCH.
#[derive(Debug)]
enum StructureValue {
    Text(String),
    List(Vec<StructureValue>),
}

fn structure_value(input: &mut &str) -> StructureValue {
    *input = input.trim_start();
    if let Some(tail) = input.strip_prefix('(') {
        *input = tail;
        let mut fields = Vec::new();
        loop {
            *input = input.trim_start();
            if let Some(tail) = input.strip_prefix(')') {
                *input = tail;
                return StructureValue::List(fields);
            }
            assert!(!input.is_empty(), "unterminated BODYSTRUCTURE");
            fields.push(structure_value(input));
        }
    }
    if let Some(tail) = input.strip_prefix('"') {
        let mut text = String::new();
        let mut escaped = false;
        for (i, c) in tail.char_indices() {
            if escaped {
                text.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                *input = &tail[i + 1..];
                return StructureValue::Text(text);
            } else {
                text.push(c);
            }
        }
        panic!("unterminated BODYSTRUCTURE string");
    }
    let end = input
        .find(|c: char| c.is_ascii_whitespace() || c == ')')
        .unwrap_or(input.len());
    assert!(end != 0);
    let text = input[..end].to_string();
    *input = &input[end..];
    StructureValue::Text(text)
}

fn structure_list(value: &StructureValue) -> &[StructureValue] {
    match value {
        StructureValue::List(fields) => fields,
        _ => panic!("expected MIME structure: {value:?}"),
    }
}

fn structure_text(value: &StructureValue) -> &str {
    match value {
        StructureValue::Text(text) => text,
        _ => panic!("expected MIME field: {value:?}"),
    }
}

fn reported_part<'a>(root: &'a StructureValue, path: &[usize]) -> &'a [StructureValue] {
    let fields = structure_list(root);
    let Some((&number, rest)) = path.split_first() else {
        return fields;
    };
    if matches!(fields.first(), Some(StructureValue::List(_))) {
        let child = &fields[number - 1];
        let child_fields = structure_list(child);
        if rest.is_empty() {
            return child_fields;
        }
        if matches!(child_fields.first(), Some(StructureValue::Text(kind)) if kind == "MESSAGE") {
            return reported_part(&child_fields[8], rest);
        }
        return reported_part(child, rest);
    }
    assert_eq!(path, [1], "single-part embedded message has only part 1");
    fields
}

fn literal_length(response: &str) -> usize {
    response
        .split_once('{')
        .unwrap()
        .1
        .split_once('}')
        .unwrap()
        .0
        .parse()
        .unwrap()
}

/// Read literals by octet count so CRLF, NUL and tag-like body text cannot
/// accidentally satisfy the response reader. All wire tests have a deadline.
async fn exchange(
    reader: &mut Reader,
    writer: &mut Writer,
    tag: &str,
    wire: &[u8],
) -> Result<Vec<u8>> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        writer.write_all(wire).await?;
        let mut out = Vec::new();
        loop {
            let mut line = Vec::new();
            anyhow::ensure!(
                reader.read_until(b'\n', &mut line).await? != 0,
                "peer closed"
            );
            let done = line.starts_with(format!("{tag} ").as_bytes());
            let literal = line
                .strip_suffix(b"}\r\n")
                .and_then(|s| s.iter().rposition(|b| *b == b'{').map(|i| &s[i + 1..]))
                .and_then(|s| std::str::from_utf8(s).ok())
                .and_then(|s| s.parse::<usize>().ok());
            out.extend_from_slice(&line);
            if let Some(n) = literal {
                let start = out.len();
                out.resize(start + n, 0);
                reader.read_exact(&mut out[start..]).await?;
            }
            if done {
                return Ok(out);
            }
        }
    })
    .await?
}

async fn command(
    reader: &mut Reader,
    writer: &mut Writer,
    tag: &str,
    args: &str,
) -> Result<String> {
    Ok(String::from_utf8_lossy(
        &exchange(reader, writer, tag, format!("{tag} {args}\r\n").as_bytes()).await?,
    )
    .into_owned())
}

async fn selected(ms: Arc<dyn MailStore>, db: Db) -> Result<(Reader, Writer)> {
    let (client, _) = spawn_session(db, ms, ImapConfig::default()).await;
    let (mut r, mut w) = login(client).await?;
    let result = command(&mut r, &mut w, "s", "SELECT INBOX").await?;
    assert!(result.contains("s OK"), "{result}");
    Ok((r, w))
}

const MIME: &[u8] = concat!(
    "From: alice@example.com\r\nSubject: MIME demand\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=outer\r\n\r\n",
    "--outer\r\nContent-Type: multipart/alternative; boundary=alt\r\n\r\n",
    "--alt\r\nContent-Type: text/plain\r\n\r\nplain\r\n",
    "--alt\r\nContent-Type: text/html\r\n\r\n<b>html</b>\r\n--alt--\r\n",
    "--outer\r\nContent-Type: application/octet-stream\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=data.bin\r\n\r\nQQBC/w==\r\n",
    "--outer\r\nContent-Type: message/rfc822\r\n\r\n",
    "From: bob@example.org\r\nSubject: Forwarded\r\nContent-Type: multipart/mixed; boundary=inner\r\n\r\n",
    "--inner\r\nContent-Type: text/plain\r\n\r\nforwarded text\r\n",
    "--inner\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nCaf=C3=A9\r\n--inner--\r\n",
    "--outer--\r\n"
).as_bytes();

#[tokio::test]
async fn thunderbird_mime_parts_on_demand_and_structure_agree() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(&ms, &mds, MIME, "MIME", Flags(0), 1_700_000_000_000)?;
    let (mut r, mut w) = selected(ms, db).await?;
    let structure = command(&mut r, &mut w, "b", "FETCH 1 BODYSTRUCTURE").await?;
    let mut structure_input = structure.split_once("BODYSTRUCTURE ").unwrap().1;
    let parsed_structure = structure_value(&mut structure_input);
    assert!(structure.contains("\"ALTERNATIVE\""), "{structure}");
    assert!(structure.contains("\"MESSAGE\" \"RFC822\""), "{structure}");
    assert!(structure.contains("\"BASE64\" 8"), "{structure}");
    for (section, expected) in [
        ("1.1", "plain"),
        ("1.2", "<b>html</b>"),
        ("2", "QQBC/w=="),
        ("3.1", "forwarded text"),
        ("3.2", "Caf=C3=A9"),
        (
            "1.MIME",
            "Content-Type: multipart/alternative; boundary=alt\r\n\r\n",
        ),
        (
            "3.2.MIME",
            "Content-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n",
        ),
        ("3.HEADER.FIELDS (Subject)", "Subject: Forwarded\r\n\r\n"),
    ] {
        let response = command(
            &mut r,
            &mut w,
            "p",
            &format!("FETCH 1 BODY.PEEK[{section}]"),
        )
        .await?;
        assert!(
            response.contains(&format!("{{{}}}\r\n{expected})", expected.len())),
            "{section}: {response}"
        );
        if section
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        {
            let path: Vec<usize> = section.split('.').map(|n| n.parse().unwrap()).collect();
            let fields = reported_part(&parsed_structure, &path);
            let (kind, subtype, encoding) = match section {
                "1.2" => ("TEXT", "HTML", "7BIT"),
                "2" => ("APPLICATION", "OCTET-STREAM", "BASE64"),
                "3.2" => ("TEXT", "PLAIN", "QUOTED-PRINTABLE"),
                _ => ("TEXT", "PLAIN", "7BIT"),
            };
            assert_eq!(structure_text(&fields[0]), kind, "{section}");
            assert_eq!(structure_text(&fields[1]), subtype, "{section}");
            assert_eq!(structure_text(&fields[5]), encoding, "{section}");
            assert_eq!(
                structure_text(&fields[6]).parse::<usize>()?,
                literal_length(&response),
                "{section}"
            );
        }
    }
    let wrapper = command(&mut r, &mut w, "p", "FETCH 1 BODY.PEEK[3]").await?;
    assert_eq!(
        structure_text(&reported_part(&parsed_structure, &[3])[6]).parse::<usize>()?,
        literal_length(&wrapper)
    );
    let response = command(&mut r, &mut w, "h", "FETCH 1 (BODY.PEEK[3.HEADER] BODY.PEEK[3.TEXT] BODY.PEEK[3.HEADER.FIELDS.NOT (From Content-Type)] BODY.PEEK[1.HEADER])").await?;
    assert!(
        response.contains("Subject: Forwarded\r\n\r\n"),
        "{response}"
    );
    assert!(response.contains("BODY[1.HEADER] NIL"), "{response}");
    let response = command(
        &mut r,
        &mut w,
        "x",
        "FETCH 1 (BODY.PEEK[2]<2.3> BODY.PEEK[99] FLAGS)",
    )
    .await?;
    assert!(response.contains("BODY[2]<2> {3}\r\nBC/"), "{response}");
    assert!(response.contains("BODY[99] NIL"), "{response}");
    assert!(
        response.contains("FLAGS ()"),
        "PEEK changed flags: {response}"
    );
    Ok(())
}

#[tokio::test]
async fn fetch_macros_and_non_extensible_body() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(&ms, &mds, MIME, "MIME", Flags(0), 1_700_000_000_000)?;
    let (mut r, mut w) = selected(ms, db).await?;
    for (macro_, envelope, body) in [
        ("FAST", false, false),
        ("ALL", true, false),
        ("FULL", true, true),
    ] {
        let result = command(&mut r, &mut w, "m", &format!("FETCH 1 {macro_}")).await?;
        assert!(result.contains("RFC822.SIZE"), "{result}");
        assert_eq!(result.contains("ENVELOPE"), envelope, "{result}");
        assert_eq!(result.contains(" BODY ("), body, "{result}");
        assert!(
            !result.contains("ATTACHMENT"),
            "BODY has extension fields: {result}"
        );
    }
    assert!(
        command(&mut r, &mut w, "b", "FETCH 1 BODY")
            .await?
            .contains("BODY ((")
    );
    assert!(
        command(&mut r, &mut w, "b", "FETCH 1 (FAST UID)")
            .await?
            .contains("BAD")
    );
    Ok(())
}

#[tokio::test]
async fn binary_decodes_sizes_partials_literal8_and_seen() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(&ms, &mds, MIME, "MIME", Flags(0), 1_700_000_000_000)?;
    let (mut r, mut w) = selected(ms, db).await?;
    let result = exchange(
        &mut r,
        &mut w,
        "b",
        b"b FETCH 1 (BINARY.PEEK[2] BINARY.SIZE[2] FLAGS)\r\n",
    )
    .await?;
    assert!(
        result
            .windows(b"BINARY[2] ~{4}\r\nA\0B\xff".len())
            .any(|s| s == b"BINARY[2] ~{4}\r\nA\0B\xff"),
        "{result:?}"
    );
    assert!(String::from_utf8_lossy(&result).contains("BINARY.SIZE[2] 4 FLAGS ()"));
    let result = command(
        &mut r,
        &mut w,
        "b",
        "FETCH 1 (BINARY.PEEK[3.2] BINARY.SIZE[3.2])",
    )
    .await?;
    assert!(
        result.contains("{5}\r\nCafé BINARY.SIZE[3.2] 5"),
        "{result}"
    );
    let result = exchange(&mut r, &mut w, "b", b"b FETCH 1 BINARY[2]<1.2>\r\n").await?;
    assert!(
        result
            .windows(b"~{2}\r\n\0B".len())
            .any(|s| s == b"~{2}\r\n\0B"),
        "{result:?}"
    );
    assert!(String::from_utf8_lossy(&result).contains("FLAGS (\\Seen)"));
    Ok(())
}

#[tokio::test]
async fn binary_unknown_and_malformed_transfer_encoding_fail() -> Result<()> {
    for (encoding, data) in [
        ("x-custom", "encoded"),
        ("base64", "a!=="),
        ("quoted-printable", "=GG"),
    ] {
        let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
        let raw = format!(
            "Content-Type: application/octet-stream\r\nContent-Transfer-Encoding: {encoding}\r\n\r\n{data}"
        );
        insert_email(
            &ms,
            &mds,
            raw.as_bytes(),
            "bad",
            Flags(0),
            1_700_000_000_000,
        )?;
        let (mut r, mut w) = selected(ms, db).await?;
        for atom in ["BINARY[1]", "BINARY.SIZE[1]"] {
            let result = command(&mut r, &mut w, "b", &format!("FETCH 1 {atom}")).await?;
            assert!(
                result.contains("b NO [UNKNOWN-CTE]"),
                "{encoding}: {result}"
            );
        }
        assert!(
            command(&mut r, &mut w, "f", "FETCH 1 FLAGS")
                .await?
                .contains("FLAGS ()")
        );
    }
    Ok(())
}

#[tokio::test]
async fn enable_rev2_esearch_save_and_dollar_survive_expunge() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    for n in 1..=3 {
        insert_email(
            &ms,
            &mds,
            b"Subject: x\r\n\r\nbody",
            &format!("m{n}"),
            Flags(0),
            1_700_000_000_000,
        )?;
    }
    let (client, _) = spawn_session(db, ms, ImapConfig::default()).await;
    let (mut r, mut w) = login(client).await?;
    let enabled = command(
        &mut r,
        &mut w,
        "e",
        "ENABLE IMAP4rev2 UTF8=ACCEPT CONDSTORE QRESYNC unknown",
    )
    .await?;
    assert!(
        enabled.starts_with("* ENABLED IMAP4rev2 UTF8=ACCEPT\r\n"),
        "{enabled}"
    );
    command(&mut r, &mut w, "s", "SELECT INBOX").await?;
    let result = command(&mut r, &mut w, "q", "SEARCH ALL").await?;
    assert!(result.contains("* ESEARCH (TAG \"q\") ALL 1:3"), "{result}");
    let result = command(
        &mut r,
        &mut w,
        "q",
        "UID SEARCH RETURN (MIN MAX ALL COUNT SAVE) ALL",
    )
    .await?;
    assert!(
        result.contains("UID MIN 1 MAX 3 ALL 1:3 COUNT 3"),
        "{result}"
    );
    command(&mut r, &mut w, "f", "UID STORE 1 +FLAGS (\\Deleted)").await?;
    command(&mut r, &mut w, "x", "UID EXPUNGE 1").await?;
    let result = command(&mut r, &mut w, "f", "FETCH $ UID").await?;
    assert!(result.contains("* 1 FETCH (UID 2)"), "{result}");
    assert!(result.contains("* 2 FETCH (UID 3)"), "{result}");
    let result = command(&mut r, &mut w, "q", "SEARCH RETURN (COUNT) $").await?;
    assert!(result.contains("COUNT 2"), "{result}");
    let result = command(&mut r, &mut w, "q", "SEARCH RETURN (SAVE) UID 999").await?;
    assert_eq!(result, "q OK SEARCH completed\r\n");
    assert_eq!(
        command(&mut r, &mut w, "f", "UID FETCH $ FLAGS").await?,
        "f OK UID FETCH completed\r\n"
    );
    let result = command(
        &mut r,
        &mut w,
        "q",
        "SEARCH RETURN (MIN MAX ALL COUNT) UID 999",
    )
    .await?;
    assert!(result.contains("* ESEARCH (TAG \"q\") COUNT 0"), "{result}");
    Ok(())
}

#[tokio::test]
async fn legacy_search_and_save_min_max_with_uid_copy_and_store() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    for n in 1..=3 {
        insert_email(
            &ms,
            &mds,
            b"Subject: x\r\n\r\nbody",
            &format!("m{n}"),
            Flags(0),
            1_700_000_000_000,
        )?;
    }
    let (mut r, mut w) = selected(ms, db).await?;
    assert!(
        command(&mut r, &mut w, "q", "SEARCH ALL")
            .await?
            .contains("* SEARCH 1 2 3")
    );
    command(&mut r, &mut w, "q", "SEARCH RETURN (MIN MAX SAVE) ALL").await?;
    let result = command(&mut r, &mut w, "f", "UID STORE $ +FLAGS (\\Flagged)").await?;
    assert_eq!(result.matches("FLAGS (\\Flagged)").count(), 2, "{result}");
    let result = command(&mut r, &mut w, "c", "UID COPY $ Archive").await?;
    assert!(result.contains("COPYUID"), "{result}");
    command(&mut r, &mut w, "s", "SELECT Archive").await?;
    assert_eq!(
        command(&mut r, &mut w, "f", "FETCH $ UID").await?,
        "f OK FETCH completed\r\n"
    );
    Ok(())
}

#[tokio::test]
async fn status_aggregates_and_appendlimit_match_config() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let raw = b"Subject: x\r\n\r\nbody";
    insert_email(
        &ms,
        &mds,
        raw,
        "a",
        Flags(Flags::DELETED),
        1_700_000_000_000,
    )?;
    insert_email(&ms, &mds, raw, "b", Flags(0), 1_700_000_000_000)?;
    let cfg = ImapConfig {
        max_literal_bytes: 12345,
        ..ImapConfig::default()
    };
    let (client, _) = spawn_session(db, ms, cfg).await;
    let (mut r, mut w) = login(client).await?;
    let caps = command(&mut r, &mut w, "c", "CAPABILITY").await?;
    assert!(caps.contains("APPENDLIMIT=12345"), "{caps}");
    let result = command(
        &mut r,
        &mut w,
        "s",
        "STATUS INBOX (MESSAGES SIZE DELETED APPENDLIMIT RECENT)",
    )
    .await?;
    assert!(
        result.contains(&format!(
            "MESSAGES 2 SIZE {} DELETED 1 APPENDLIMIT 12345 RECENT 0",
            raw.len() * 2
        )),
        "{result}"
    );
    assert!(
        command(&mut r, &mut w, "s", "STATUS INBOX (HIGHESTMODSEQ)")
            .await?
            .contains("s BAD")
    );
    Ok(())
}

#[tokio::test]
async fn list_extended_references_patterns_status_and_special_use() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (mut r, mut w) = selected(ms, db).await?;
    command(&mut r, &mut w, "c", "CREATE Projects/One").await?;
    command(&mut r, &mut w, "c", "SUBSCRIBE Projects/One").await?;
    let result = command(
        &mut r,
        &mut w,
        "l",
        "LIST (SUBSCRIBED RECURSIVEMATCH) \"\" % RETURN (SUBSCRIBED CHILDREN)",
    )
    .await?;
    assert!(
        result.contains("\"Projects\" (CHILDINFO (\"SUBSCRIBED\"))"),
        "{result}"
    );
    let result = command(
        &mut r,
        &mut w,
        "l",
        "LIST \"Projects/\" % RETURN (STATUS (MESSAGES SIZE APPENDLIMIT) CHILDREN)",
    )
    .await?;
    assert!(result.contains("\"Projects/One\""), "{result}");
    assert!(
        result.contains("* STATUS \"Projects/One\" (MESSAGES 0 SIZE 0 APPENDLIMIT"),
        "{result}"
    );
    let result = command(&mut r, &mut w, "l", "LSUB \"Projects/\" *").await?;
    assert!(result.contains("\"Projects/One\""), "{result}");
    let result = command(
        &mut r,
        &mut w,
        "l",
        "LIST (REMOTE SPECIAL-USE) \"\" (INBOX Archive) RETURN (SPECIAL-USE)",
    )
    .await?;
    assert!(result.contains("\\Archive"), "{result}");
    assert!(result.contains("\\Inbox"), "{result}");
    assert!(!result.contains("Projects"), "{result}");
    assert!(
        command(&mut r, &mut w, "l", "LIST (RECURSIVEMATCH) \"\" *")
            .await?
            .contains("l BAD")
    );
    assert!(
        command(&mut r, &mut w, "l", "LIST \"\" * RETURN (BOGUS)")
            .await?
            .contains("l BAD")
    );
    Ok(())
}

#[tokio::test]
async fn utf8_mailbox_names_and_rev1_modified_utf7_roundtrip() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (client, _) = spawn_session(db, ms, ImapConfig::default()).await;
    let (mut r, mut w) = login(client).await?;
    assert!(
        command(&mut r, &mut w, "c", "CREATE \"Caf&AOk- &- tea\"")
            .await?
            .contains("c OK")
    );
    let result = command(&mut r, &mut w, "l", "LIST \"\" *").await?;
    assert!(result.contains("Caf&AOk- &- tea"), "{result}");
    command(&mut r, &mut w, "e", "ENABLE UTF8=ACCEPT").await?;
    let result = command(&mut r, &mut w, "l", "LIST \"\" \"Café & tea\"").await?;
    assert!(result.contains("\"Café & tea\""), "{result}");
    assert!(
        command(&mut r, &mut w, "s", "SELECT \"Café & tea\"")
            .await?
            .contains("s OK")
    );
    assert!(
        command(&mut r, &mut w, "c", "CREATE \"日本語\"")
            .await?
            .contains("c OK")
    );
    Ok(())
}

#[tokio::test]
async fn general_literals_login_names_flags_search_and_multiappend() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (client, _) = spawn_session(db, ms, ImapConfig::default()).await;
    let (rr, mut w) = tokio::io::split(client);
    let mut r = BufReader::new(rr);
    read_line(&mut r).await?;
    let result = exchange(
        &mut r,
        &mut w,
        "a",
        b"a LOGIN {17+}\r\nalice@example.com {7+}\r\nhunter2\r\n",
    )
    .await?;
    assert!(String::from_utf8_lossy(&result).contains("a OK"));
    let result = exchange(&mut r, &mut w, "c", b"c CREATE {5+}\r\nBox A\r\n").await?;
    assert!(String::from_utf8_lossy(&result).contains("c OK"));
    assert!(
        String::from_utf8_lossy(
            &exchange(
                &mut r,
                &mut w,
                "n",
                b"n RENAME {5+}\r\nBox A {5+}\r\nBox B\r\n"
            )
            .await?
        )
        .contains("n OK")
    );
    let one = b"Subject: One\r\n\r\nfirst";
    let two = b"Subject: Two\r\n\r\nsecond\0data";
    let mut wire = format!(
        "a APPEND {{5+}}\r\nBox B (\\Seen project) \"01-Jan-2026 10:00:00 +0000\" {{{}+}}\r\n",
        one.len()
    )
    .into_bytes();
    wire.extend_from_slice(one);
    wire.extend_from_slice(format!(" (\\Flagged) ~{{{}+}}\r\n", two.len()).as_bytes());
    wire.extend_from_slice(two);
    wire.extend_from_slice(b"\r\n");
    let result = exchange(&mut r, &mut w, "a", &wire).await?;
    assert!(
        String::from_utf8_lossy(&result).contains("APPENDUID"),
        "{result:?}"
    );
    assert!(
        String::from_utf8_lossy(&result).contains(" 1:2]"),
        "{result:?}"
    );
    command(&mut r, &mut w, "s", "SELECT \"Box B\"").await?;
    let result = exchange(
        &mut r,
        &mut w,
        "f",
        b"f STORE 2 +FLAGS ({6+}\r\nurgent)\r\n",
    )
    .await?;
    assert!(
        String::from_utf8_lossy(&result).contains("urgent"),
        "{result:?}"
    );
    let result = exchange(&mut r, &mut w, "q", b"q SEARCH SUBJECT {3+}\r\nTwo\r\n").await?;
    assert!(String::from_utf8_lossy(&result).contains("* SEARCH 2"));
    command(&mut r, &mut w, "u", "UNSELECT").await?;
    assert!(
        String::from_utf8_lossy(
            &exchange(&mut r, &mut w, "d", b"d DELETE {5+}\r\nBox B\r\n").await?
        )
        .contains("d OK")
    );
    Ok(())
}

#[tokio::test]
async fn multiappend_failure_leaves_no_partial_messages() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (mut r, mut w) = selected(ms, db).await?;
    let one = b"Subject: One\r\n\r\nfirst";
    let mut wire = format!("a APPEND INBOX {{{}+}}\r\n", one.len()).into_bytes();
    wire.extend_from_slice(one);
    wire.extend_from_slice(b" {0+}\r\n\r\n");
    let result = exchange(&mut r, &mut w, "a", &wire).await?;
    assert!(
        String::from_utf8_lossy(&result).contains("a NO"),
        "{result:?}"
    );
    assert!(
        command(&mut r, &mut w, "s", "STATUS INBOX (MESSAGES)")
            .await?
            .contains("MESSAGES 0")
    );
    Ok(())
}

#[tokio::test]
async fn rename_inbox_preserves_identity_children_and_moves_messages() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(
        &ms,
        &mds,
        MIME,
        "message",
        Flags(Flags::FLAGGED),
        1_700_000_000_000,
    )?;
    let (mut r, mut w) = selected(ms, db).await?;
    command(&mut r, &mut w, "c", "CREATE INBOX/Child").await?;
    command(&mut r, &mut w, "u", "UNSELECT").await?;
    let before = command(&mut r, &mut w, "b", "STATUS INBOX (UIDVALIDITY UIDNEXT)").await?;
    assert!(
        command(&mut r, &mut w, "n", "RENAME INBOX Saved")
            .await?
            .contains("n OK")
    );
    let after = command(&mut r, &mut w, "b", "STATUS INBOX (UIDVALIDITY UIDNEXT)").await?;
    assert_eq!(before, after);
    assert!(
        command(&mut r, &mut w, "b", "STATUS INBOX (MESSAGES)")
            .await?
            .contains("MESSAGES 0")
    );
    assert!(
        command(&mut r, &mut w, "s", "SELECT INBOX/Child")
            .await?
            .contains("s OK")
    );
    command(&mut r, &mut w, "s", "SELECT Saved").await?;
    assert!(
        command(&mut r, &mut w, "f", "FETCH 1 FLAGS")
            .await?
            .contains("\\Flagged")
    );
    Ok(())
}

#[tokio::test]
async fn examine_fetch_and_close_unselect_have_distinct_effects() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(
        &ms,
        &mds,
        b"Subject: x\r\n\r\nbody",
        "a",
        Flags(Flags::DELETED),
        1_700_000_000_000,
    )?;
    let (mut r, mut w) = selected(ms, db).await?;
    command(&mut r, &mut w, "s", "EXAMINE INBOX").await?;
    let result = command(&mut r, &mut w, "f", "FETCH 1 (BODY[] FLAGS)").await?;
    assert!(
        !result.contains("\\Seen"),
        "EXAMINE changed flags: {result}"
    );
    command(&mut r, &mut w, "c", "CLOSE").await?;
    assert!(
        command(&mut r, &mut w, "s", "STATUS INBOX (MESSAGES)")
            .await?
            .contains("MESSAGES 1")
    );
    command(&mut r, &mut w, "s", "SELECT INBOX").await?;
    command(&mut r, &mut w, "u", "UNSELECT").await?;
    assert!(
        command(&mut r, &mut w, "s", "STATUS INBOX (MESSAGES)")
            .await?
            .contains("MESSAGES 1")
    );
    command(&mut r, &mut w, "s", "SELECT INBOX").await?;
    assert_eq!(
        command(&mut r, &mut w, "c", "CLOSE").await?,
        "c OK CLOSE completed\r\n"
    );
    assert!(
        command(&mut r, &mut w, "s", "STATUS INBOX (MESSAGES)")
            .await?
            .contains("MESSAGES 0")
    );
    Ok(())
}

#[tokio::test]
async fn copy_failure_rolls_back_earlier_destinations_and_uidnext() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    for n in 1..=2 {
        insert_email(
            &ms,
            &mds,
            b"Subject: x\r\n\r\nbody",
            &format!("m{n}"),
            Flags(0),
            1_700_000_000_000,
        )?;
    }
    let (mut r, mut w) = selected(ms, db).await?;
    command(&mut r, &mut w, "c", "COPY 2 Archive").await?;
    let before = command(&mut r, &mut w, "s", "STATUS Archive (MESSAGES UIDNEXT)").await?;
    assert!(
        command(&mut r, &mut w, "c", "COPY 1:2 Archive")
            .await?
            .contains("c NO")
    );
    assert_eq!(
        before,
        command(&mut r, &mut w, "s", "STATUS Archive (MESSAGES UIDNEXT)").await?
    );
    Ok(())
}

#[tokio::test]
async fn multiappend_storage_failure_rolls_back_all_rows_and_uids() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let set = maild::mailstore::account_id_to_setid(1);
    mds.with_set_tx(&set, |tx| {
        tx.tx().execute_batch("CREATE TRIGGER reject_append BEFORE INSERT ON mail_envelopes WHEN NEW.subject = 'reject' BEGIN SELECT RAISE(ABORT, 'injected append failure'); END")
            .map_err(|e| mds::Error::Other(e.to_string()))?;
        Ok(())
    })?;
    let (mut r, mut w) = selected(ms, db).await?;
    let before = command(&mut r, &mut w, "s", "STATUS INBOX (MESSAGES UIDNEXT)").await?;
    let first = b"Subject: first\r\n\r\nbody";
    let second = b"Subject: reject\r\n\r\nbody";
    let mut wire = format!("a APPEND INBOX {{{}+}}\r\n", first.len()).into_bytes();
    wire.extend_from_slice(first);
    wire.extend_from_slice(format!(" {{{}+}}\r\n", second.len()).as_bytes());
    wire.extend_from_slice(second);
    wire.extend_from_slice(b"\r\n");
    assert!(String::from_utf8_lossy(&exchange(&mut r, &mut w, "a", &wire).await?).contains("a NO"));
    assert_eq!(
        before,
        command(&mut r, &mut w, "s", "STATUS INBOX (MESSAGES UIDNEXT)").await?
    );
    Ok(())
}

#[tokio::test]
async fn shutdown_sends_bye_during_idle_and_releases_account_slot() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (client, server) = tokio::io::duplex(65536);
    let (shutdown, rx) = tokio::sync::watch::channel(false);
    let slots = AccountSlots::new();
    let session_slots = slots.clone();
    let join = tokio::spawn(async move {
        maild::imap::session::handle_connection_with_shutdown(
            server,
            "127.0.0.1:0".parse().unwrap(),
            None,
            Arc::new(ImapConfig::default()),
            db,
            ms,
            session_slots,
            "localhost".into(),
            Some(rx),
        )
        .await
    });
    let (mut r, mut w) = login(client).await?;
    w.write_all(b"i IDLE\r\n").await?;
    assert_eq!(read_line(&mut r).await?, "+ idling");
    shutdown.send(true)?;
    let bye = tokio::time::timeout(std::time::Duration::from_secs(5), read_line(&mut r)).await??;
    assert!(bye.starts_with("* BYE [UNAVAILABLE]"), "{bye}");
    join.await??;
    assert!(slots.snapshot().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn subscriptions_survive_rename_and_delete_and_absent_names_can_be_removed() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (mut r, mut w) = selected(ms, db).await?;
    command(&mut r, &mut w, "c", "CREATE Original").await?;
    command(&mut r, &mut w, "s", "SUBSCRIBE Original").await?;
    command(&mut r, &mut w, "n", "RENAME Original Renamed").await?;
    let result = command(&mut r, &mut w, "l", "LIST (SUBSCRIBED) \"\" Original").await?;
    assert!(
        result.contains("\\Subscribed") && result.contains("\\NonExistent"),
        "{result}"
    );
    command(&mut r, &mut w, "d", "DELETE Renamed").await?;
    let result = command(&mut r, &mut w, "l", "LSUB \"\" Renamed").await?;
    assert!(result.contains("Renamed"), "{result}");
    command(&mut r, &mut w, "u", "UNSUBSCRIBE Original").await?;
    assert_eq!(
        command(&mut r, &mut w, "l", "LSUB \"\" Original").await?,
        "l OK LSUB completed\r\n"
    );
    Ok(())
}

#[tokio::test]
async fn pending_expunge_preserves_msns_until_a_permitted_command() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    for n in 1..=2 {
        insert_email(
            &ms,
            &mds,
            b"Subject: x\r\n\r\nbody",
            &format!("m{n}"),
            Flags(0),
            1_700_000_000_000,
        )?;
    }
    let (mut r, mut w) = selected(ms.clone(), db).await?;
    let inbox = ms.mailbox_by_role(1, MailboxRole::Inbox)?.unwrap();
    let first = ms.list_emails_in_mailbox(
        1,
        inbox,
        maild::mailstore::ListOpts {
            sort_by: maild::mailstore::SortKey::Seq,
            limit: 1,
            offset: 0,
        },
    )?[0]
        .id;
    ms.expunge_memberships(1, inbox, &[first])?;
    let result = command(&mut r, &mut w, "f", "FETCH 2 UID").await?;
    assert!(result.contains("* 2 FETCH (UID 2)"), "{result}");
    assert!(!result.contains("EXPUNGE"), "{result}");
    let result = command(&mut r, &mut w, "f", "STORE 2 +FLAGS (\\Flagged)").await?;
    assert!(result.contains("* 2 FETCH"), "{result}");
    assert!(!result.contains("EXPUNGE"), "{result}");
    let result = command(&mut r, &mut w, "q", "SEARCH ALL").await?;
    assert!(result.contains("* SEARCH 2"), "{result}");
    let result = command(&mut r, &mut w, "n", "NOOP").await?;
    assert!(result.contains("* 1 EXPUNGE"), "{result}");
    assert!(
        command(&mut r, &mut w, "f", "FETCH 1 UID")
            .await?
            .contains("* 1 FETCH (UID 2)")
    );
    insert_email(
        &ms,
        &mds,
        b"Subject: new\r\n\r\nbody",
        "new",
        Flags(0),
        1_700_000_000_000,
    )?;
    assert!(
        command(&mut r, &mut w, "n", "NOOP")
            .await?
            .contains("* 2 EXISTS")
    );
    Ok(())
}

#[tokio::test]
async fn utf8_append_wrapper_and_literal_kind_are_checked() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (client, _) = spawn_session(db, ms, ImapConfig::default()).await;
    let (mut r, mut w) = login(client).await?;
    command(&mut r, &mut w, "e", "ENABLE UTF8=ACCEPT").await?;
    let raw = "Subject: Café\r\n\r\ntext".as_bytes();
    let mut wire = format!("a APPEND INBOX UTF8 (~{{{}+}}\r\n", raw.len()).into_bytes();
    wire.extend_from_slice(raw);
    wire.extend_from_slice(b")\r\n");
    assert!(
        String::from_utf8_lossy(&exchange(&mut r, &mut w, "a", &wire).await?)
            .contains("a OK [APPENDUID")
    );
    let raw = b"Subject: x\r\n\r\n\0";
    let mut wire = format!("a APPEND INBOX {{{}+}}\r\n", raw.len()).into_bytes();
    wire.extend_from_slice(raw);
    wire.extend_from_slice(b"\r\n");
    assert!(
        String::from_utf8_lossy(&exchange(&mut r, &mut w, "a", &wire).await?).contains("a BAD")
    );
    let raw = b"Subject: binary\r\nContent-Type: application/octet-stream\r\nContent-Transfer-Encoding: binary\r\n\r\nA\0B\xff";
    let mut wire = format!("a APPEND INBOX ~{{{}+}}\r\n", raw.len()).into_bytes();
    wire.extend_from_slice(raw);
    wire.extend_from_slice(b"\r\n");
    assert!(
        String::from_utf8_lossy(&exchange(&mut r, &mut w, "a", &wire).await?)
            .contains("a OK [APPENDUID")
    );
    command(&mut r, &mut w, "s", "SELECT INBOX").await?;
    let result = command(&mut r, &mut w, "f", "FETCH 2 BODY.PEEK[1]").await?;
    assert!(result.contains("{8}\r\nQQBC/w=="), "{result}");
    let result = exchange(&mut r, &mut w, "f", b"f FETCH 2 BINARY.PEEK[1]\r\n").await?;
    assert!(
        result
            .windows(b"~{4}\r\nA\0B\xff".len())
            .any(|s| s == b"~{4}\r\nA\0B\xff"),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn binary_text_normalises_crlf_before_sizing_and_slicing() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(
        &ms,
        &mds,
        b"Content-Type: text/plain\n\nline1\nline2\n",
        "lf",
        Flags(0),
        1_700_000_000_000,
    )?;
    let (mut r, mut w) = selected(ms, db).await?;
    let result = command(
        &mut r,
        &mut w,
        "f",
        "FETCH 1 (BINARY.PEEK[1] BINARY.SIZE[1] BINARY.PEEK[1]<5.4>)",
    )
    .await?;
    assert!(
        result.contains("{14}\r\nline1\r\nline2\r\n BINARY.SIZE[1] 14"),
        "{result}"
    );
    assert!(result.contains("BINARY[1]<5> {4}\r\n\r\nli"), "{result}");
    Ok(())
}

#[tokio::test]
async fn literal_minus_limits_only_non_synchronising_literals() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let cfg = ImapConfig {
        advertise_capabilities: Some(vec![
            "IMAP4rev2".into(),
            "IMAP4rev1".into(),
            "LITERAL-".into(),
        ]),
        ..Default::default()
    };
    let (client, _) = spawn_session(db, ms.clone(), cfg).await;
    let (mut r, mut w) = login(client).await?;
    let caps = command(&mut r, &mut w, "c", "CAPABILITY").await?;
    assert!(
        caps.contains("LITERAL-") && !caps.contains("LITERAL+"),
        "{caps}"
    );
    let mut raw = b"Subject: large\r\n\r\n".to_vec();
    raw.resize(5000, b'a');
    w.write_all(b"a APPEND INBOX {5000}\r\n").await?;
    assert_eq!(read_line(&mut r).await?, "+ Ready for literal data");
    w.write_all(&raw).await?;
    w.write_all(b"\r\n").await?;
    assert!(
        read_until_tagged(&mut r, "a")
            .await?
            .last()
            .unwrap()
            .contains("a OK [APPENDUID")
    );
    let mut wire = b"a APPEND INBOX {5000+}\r\n".to_vec();
    wire.extend_from_slice(&raw);
    wire.extend_from_slice(b"\r\nevil DELETE Archive\r\n");
    let result = exchange(&mut r, &mut w, "a", &wire).await?;
    assert!(
        String::from_utf8_lossy(&result).contains("a BAD"),
        "{result:?}"
    );
    assert!(read_line(&mut r).await?.starts_with("* BYE"));
    assert!(ms.mailbox_by_role(1, MailboxRole::Archive)?.is_some());
    Ok(())
}

#[tokio::test]
async fn extended_list_empty_patterns_lsub_ancestors_and_hostile_wildcards() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (mut r, mut w) = selected(ms, db).await?;
    assert_eq!(
        command(&mut r, &mut w, "l", "LIST () INBOX \"\" RETURN (CHILDREN)").await?,
        "l OK LIST completed\r\n"
    );
    assert_eq!(
        command(&mut r, &mut w, "l", "LIST \"\" (\"\")").await?,
        "l OK LIST completed\r\n"
    );
    command(&mut r, &mut w, "s", "SUBSCRIBE Ghost/Child").await?;
    let result = command(&mut r, &mut w, "l", "LSUB \"\" %").await?;
    assert!(
        result.contains("* LSUB (\\Noselect) \"/\" \"Ghost\""),
        "{result}"
    );
    let result = command(
        &mut r,
        &mut w,
        "l",
        "LIST (SUBSCRIBED RECURSIVEMATCH SUBSCRIBED) \"\" %",
    )
    .await?;
    assert!(
        result.contains("\"Ghost\" (CHILDINFO (\"SUBSCRIBED\"))"),
        "{result}"
    );
    let name = "a".repeat(100);
    command(&mut r, &mut w, "c", &format!("CREATE {name}")).await?;
    let pattern = "*a".repeat(50) + "b";
    assert_eq!(
        command(&mut r, &mut w, "l", &format!("LIST \"\" {pattern}")).await?,
        "l OK LIST completed\r\n"
    );
    Ok(())
}

#[tokio::test]
async fn charset_errors_and_save_failure_have_correct_state() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(
        &ms,
        &mds,
        b"Subject: x\r\n\r\nbody",
        "x",
        Flags(0),
        1_700_000_000_000,
    )?;
    let (mut r, mut w) = selected(ms, db).await?;
    command(&mut r, &mut w, "q", "SEARCH RETURN (SAVE) ALL").await?;
    assert!(
        command(
            &mut r,
            &mut w,
            "q",
            "SEARCH RETURN (SAVE) CHARSET X-UNKNOWN ALL"
        )
        .await?
        .contains("q NO [BADCHARSET (US-ASCII UTF-8)]")
    );
    assert_eq!(
        command(&mut r, &mut w, "f", "FETCH $ UID").await?,
        "f OK FETCH completed\r\n"
    );
    command(&mut r, &mut w, "u", "UNSELECT").await?;
    command(&mut r, &mut w, "e", "ENABLE IMAP4rev2").await?;
    command(&mut r, &mut w, "s", "SELECT INBOX").await?;
    assert!(
        command(&mut r, &mut w, "q", "SEARCH CHARSET UTF-8 ALL")
            .await?
            .contains("q BAD")
    );
    Ok(())
}

#[tokio::test]
async fn search_work_limit_never_returns_a_truncated_match_set() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(
        &ms,
        &mds,
        b"Subject: x\r\n\r\nbody",
        "x",
        Flags(0),
        1_700_000_000_000,
    )?;
    // Exercise the metadata work guard without allocating a giant fixture.
    mds.with_set_tx(&maild::mailstore::account_id_to_setid(1), |tx| {
        tx.tx()
            .execute("UPDATE item SET size_bytes = 67108865", [])
            .map_err(|e| mds::Error::Other(e.to_string()))?;
        Ok(())
    })?;
    let (mut r, mut w) = selected(ms, db).await?;
    assert!(
        command(&mut r, &mut w, "q", "SEARCH ALL")
            .await?
            .contains("* SEARCH 1")
    );
    for criteria in ["BODY absent", "NOT BODY absent", "RETURN (SAVE) TEXT body"] {
        let result = command(&mut r, &mut w, "q", &format!("SEARCH {criteria}")).await?;
        assert!(result.contains("q NO [LIMIT]"), "{criteria}: {result}");
        assert!(!result.contains("* SEARCH"), "{result}");
    }
    assert_eq!(
        command(&mut r, &mut w, "f", "FETCH $ UID").await?,
        "f OK FETCH completed\r\n"
    );
    Ok(())
}

#[tokio::test]
async fn capabilities_match_after_login_and_enable_and_zero_arg_commands_validate() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (client, _) = spawn_session(db, ms, ImapConfig::default()).await;
    let (rr, mut w) = tokio::io::split(client);
    let mut r = BufReader::new(rr);
    let greeting = read_line(&mut r).await?;
    let caps = greeting
        .split("[CAPABILITY ")
        .nth(1)
        .unwrap()
        .split(']')
        .next()
        .unwrap()
        .to_string();
    w.write_all(b"a LOGIN alice@example.com hunter2\r\n")
        .await?;
    let authenticated = read_line(&mut r).await?;
    assert!(
        authenticated.contains(&format!("[CAPABILITY {caps}]")),
        "{authenticated}"
    );
    command(&mut r, &mut w, "e", "ENABLE IMAP4rev2").await?;
    let result = command(&mut r, &mut w, "c", "CAPABILITY").await?;
    assert!(
        result.contains(&format!("* CAPABILITY {caps}\r\n")),
        "{result}"
    );
    for command_name in ["CAPABILITY", "NOOP", "NAMESPACE", "LOGOUT"] {
        assert!(
            command(&mut r, &mut w, "b", &format!("{command_name} extra"))
                .await?
                .contains("b BAD")
        );
    }
    Ok(())
}

#[tokio::test]
async fn append_and_inbox_rename_report_observed_size_changes_before_completion() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (mut r, mut w) = selected(ms, db).await?;
    let raw = b"Subject: x\r\n\r\nbody";
    let mut wire = format!("a APPEND INBOX {{{}+}}\r\n", raw.len()).into_bytes();
    wire.extend_from_slice(raw);
    wire.extend_from_slice(b"\r\n");
    let response = exchange(&mut r, &mut w, "a", &wire).await?;
    assert!(
        response.starts_with(b"* 1 EXISTS\r\na OK [APPENDUID"),
        "{response:?}"
    );
    let response = command(&mut r, &mut w, "n", "RENAME INBOX Saved").await?;
    assert!(response.starts_with("* 1 EXPUNGE\r\nn OK"), "{response}");
    Ok(())
}

#[tokio::test]
async fn native_jmap_name_and_subscription_updates_keep_their_contracts() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(
        &ms,
        &mds,
        b"Subject: x\r\n\r\nbody",
        "x",
        Flags(0),
        1_700_000_000_000,
    )?;
    let inbox = ms.mailbox_by_role(1, MailboxRole::Inbox)?.unwrap();
    ms.rename_mailbox(1, inbox, "Display name", None)?;
    let record = ms
        .list_mailboxes(1)?
        .into_iter()
        .find(|r| r.id == inbox)
        .unwrap();
    assert_eq!(record.name, "Display name");
    assert_eq!(record.total_emails, 1);
    let (mut r, mut w) = selected(ms.clone(), db).await?;
    command(&mut r, &mut w, "c", "CREATE Subscribed").await?;
    command(&mut r, &mut w, "s", "SUBSCRIBE Subscribed").await?;
    let id = ms
        .list_mailboxes(1)?
        .into_iter()
        .find(|r| r.name == "Subscribed")
        .unwrap()
        .id;
    ms.update_mailbox(1, id, None, None, None, None, Some(false))?;
    assert_eq!(
        command(&mut r, &mut w, "l", "LIST (SUBSCRIBED) \"\" Subscribed").await?,
        "l OK LIST completed\r\n"
    );
    Ok(())
}

#[tokio::test]
async fn idle_reports_flag_changes_with_uids_without_modseq() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(
        &ms,
        &mds,
        b"Subject: x\r\n\r\nbody",
        "x",
        Flags(0),
        1_700_000_000_000,
    )?;
    let (mut r, mut w) = selected(ms.clone(), db).await?;
    let inbox = ms.mailbox_by_role(1, MailboxRole::Inbox)?.unwrap();
    let item = ms.list_emails_in_mailbox(1, inbox, maild::mailstore::ListOpts::default())?[0].id;
    w.write_all(b"i IDLE\r\n").await?;
    assert_eq!(read_line(&mut r).await?, "+ idling");
    ms.add_flags_in_mailbox(1, item, inbox, Flags(Flags::FLAGGED))?;
    let response =
        tokio::time::timeout(std::time::Duration::from_secs(5), read_line(&mut r)).await??;
    assert_eq!(response, "* 1 FETCH (UID 1 FLAGS (\\Flagged))");
    w.write_all(b"DONE\r\n").await?;
    assert!(read_line(&mut r).await?.starts_with("i OK"));
    Ok(())
}

#[tokio::test]
async fn unrepresentable_native_names_cannot_inject_subscription_responses() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let bad = ms.create_mailbox(
        1,
        "bad\r\n* CAPABILITY INJECTED",
        None,
        attrs_for(None, true),
    )?;
    let (mut r, mut w) = selected(ms.clone(), db).await?;
    command(&mut r, &mut w, "c", "CREATE Safe").await?;
    assert!(
        command(&mut r, &mut w, "n", "RENAME Safe SafeAgain")
            .await?
            .contains("n OK")
    );
    let response = command(&mut r, &mut w, "l", "LIST (SUBSCRIBED) \"\" *").await?;
    assert!(
        !response.contains("INJECTED") && !response.contains("bad"),
        "{response}"
    );
    ms.update_mailbox(1, bad, None, None, None, None, Some(false))?;
    command(&mut r, &mut w, "s", "SUBSCRIBE inbox").await?;
    let response = command(&mut r, &mut w, "l", "LSUB \"\" *").await?;
    assert_eq!(response.matches("\"INBOX\"").count(), 1, "{response}");
    assert!(!response.contains("\"inbox\""), "{response}");
    Ok(())
}

#[tokio::test]
async fn uidvalidity_does_not_repeat_on_rapid_or_legacy_mailbox_recreation() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (mut r, mut w) = selected(ms.clone(), db.clone()).await?;
    command(&mut r, &mut w, "c", "CREATE Identity").await?;
    let id = ms
        .list_mailboxes(1)?
        .into_iter()
        .find(|record| record.name == "Identity")
        .unwrap()
        .id;
    // Model an existing legacy mailbox whose timestamp is ahead of the wall
    // clock; deletion must retain that high-water mark too.
    let legacy = (chrono::Utc::now().timestamp() + 3600) as u64;
    mds.with_set_tx(&maild::mailstore::account_id_to_setid(1), |tx| {
        tx.tx()
            .execute(
                "UPDATE container SET seq_validity = ?1 WHERE id = ?2",
                params![legacy * 1000, id.0.to_string()],
            )
            .map_err(|e| mds::Error::Other(e.to_string()))?;
        Ok(())
    })?;
    let mut previous = legacy;
    for _ in 0..3 {
        assert!(
            command(&mut r, &mut w, "d", "DELETE Identity")
                .await?
                .contains("d OK")
        );
        assert!(
            command(&mut r, &mut w, "c", "CREATE Identity")
                .await?
                .contains("c OK")
        );
        let response = command(&mut r, &mut w, "s", "STATUS Identity (UIDVALIDITY)").await?;
        let value: u64 = response
            .split("UIDVALIDITY ")
            .nth(1)
            .unwrap()
            .split(')')
            .next()
            .unwrap()
            .parse()?;
        assert!(
            value > previous && value <= u32::MAX as u64,
            "{previous} -> {response}"
        );
        previous = value;
    }
    let (client, _) = spawn_session(db, ms, ImapConfig::default()).await;
    let (mut r2, mut w2) = login(client).await?;
    command(&mut r2, &mut w2, "d", "DELETE Identity").await?;
    command(&mut r2, &mut w2, "c", "CREATE Identity").await?;
    let response = command(&mut r2, &mut w2, "s", "STATUS Identity (UIDVALIDITY)").await?;
    let value: u64 = response
        .split("UIDVALIDITY ")
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap()
        .parse()?;
    assert!(value > previous, "{response}");
    Ok(())
}

#[tokio::test]
async fn binary_quoted_printable_discards_transport_padding_and_keeps_encoded_spaces() -> Result<()>
{
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let raw = b"Content-Type: application/octet-stream\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n=41 \t\r\nline=20 \t\r\n=00";
    insert_email(&ms, &mds, raw, "qp", Flags(0), 1_700_000_000_000)?;
    let (mut r, mut w) = selected(ms, db).await?;
    let result = exchange(
        &mut r,
        &mut w,
        "f",
        b"f FETCH 1 (BINARY.PEEK[1] BINARY.SIZE[1])\r\n",
    )
    .await?;
    assert!(
        result
            .windows(b"~{11}\r\nA\r\nline \r\n\0 BINARY.SIZE[1] 11".len())
            .any(|part| part == b"~{11}\r\nA\r\nline \r\n\0 BINARY.SIZE[1] 11"),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn fatal_selected_mailbox_loss_emits_bye() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let id = ms.create_mailbox(1, "Temporary", None, attrs_for(None, false))?;
    let (mut r, mut w) = selected(ms.clone(), db).await?;
    command(&mut r, &mut w, "s", "SELECT Temporary").await?;
    ms.destroy_mailbox(1, id, maild::mailstore::DestroyPolicy::Delete)?;
    w.write_all(b"n NOOP\r\n").await?;
    let response =
        tokio::time::timeout(std::time::Duration::from_secs(5), read_line(&mut r)).await??;
    assert!(response.starts_with("* BYE [UNAVAILABLE]"), "{response}");
    Ok(())
}
