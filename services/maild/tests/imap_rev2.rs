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

/// Read literals by octet count so CRLF, NUL and tag-like body text cannot
/// accidentally satisfy the response reader. All wire tests have a deadline.
async fn exchange(reader: &mut Reader, writer: &mut Writer, tag: &str, wire: &[u8]) -> Result<Vec<u8>> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        writer.write_all(wire).await?;
        let mut out = Vec::new();
        loop {
            let mut line = Vec::new();
            anyhow::ensure!(reader.read_until(b'\n', &mut line).await? != 0, "peer closed");
            let done = line.starts_with(format!("{tag} ").as_bytes());
            let literal = line.strip_suffix(b"}\r\n").and_then(|s| s.iter().rposition(|b| *b == b'{').map(|i| &s[i+1..])).and_then(|s| std::str::from_utf8(s).ok()).and_then(|s| s.parse::<usize>().ok());
            out.extend_from_slice(&line);
            if let Some(n) = literal {
                let start = out.len();
                out.resize(start+n, 0);
                reader.read_exact(&mut out[start..]).await?;
            }
            if done { return Ok(out); }
        }
    }).await?
}

async fn command(reader: &mut Reader, writer: &mut Writer, tag: &str, args: &str) -> Result<String> {
    Ok(String::from_utf8_lossy(&exchange(reader, writer, tag, format!("{tag} {args}\r\n").as_bytes()).await?).into_owned())
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
    assert!(structure.contains("\"ALTERNATIVE\""), "{structure}");
    assert!(structure.contains("\"MESSAGE\" \"RFC822\""), "{structure}");
    assert!(structure.contains("\"BASE64\" 8"), "{structure}");
    for (section, expected) in [
        ("1.1", "plain"), ("1.2", "<b>html</b>"), ("2", "QQBC/w=="),
        ("3.1", "forwarded text"), ("3.2", "Caf=C3=A9"),
        ("1.MIME", "Content-Type: multipart/alternative; boundary=alt\r\n\r\n"),
        ("3.2.MIME", "Content-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n"),
        ("3.HEADER.FIELDS (Subject)", "Subject: Forwarded\r\n\r\n"),
    ] {
        let response = command(&mut r, &mut w, "p", &format!("FETCH 1 BODY.PEEK[{section}]")).await?;
        assert!(response.contains(&format!("{{{}}}\r\n{expected})", expected.len())), "{section}: {response}");
    }
    let response = command(&mut r, &mut w, "h", "FETCH 1 (BODY.PEEK[3.HEADER] BODY.PEEK[3.TEXT] BODY.PEEK[3.HEADER.FIELDS.NOT (From Content-Type)] BODY.PEEK[1.HEADER])").await?;
    assert!(response.contains("Subject: Forwarded\r\n\r\n"), "{response}");
    assert!(response.contains("BODY[1.HEADER] NIL"), "{response}");
    let response = command(&mut r, &mut w, "x", "FETCH 1 (BODY.PEEK[2]<2.3> BODY.PEEK[99] FLAGS)").await?;
    assert!(response.contains("BODY[2]<2> {3}\r\nBC/"), "{response}");
    assert!(response.contains("BODY[99] NIL"), "{response}");
    assert!(response.contains("FLAGS ()"), "PEEK changed flags: {response}");
    Ok(())
}

#[tokio::test]
async fn fetch_macros_and_non_extensible_body() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(&ms, &mds, MIME, "MIME", Flags(0), 1_700_000_000_000)?;
    let (mut r, mut w) = selected(ms, db).await?;
    for (macro_, envelope, body) in [("FAST", false, false), ("ALL", true, false), ("FULL", true, true)] {
        let result = command(&mut r, &mut w, "m", &format!("FETCH 1 {macro_}")).await?;
        assert!(result.contains("RFC822.SIZE"), "{result}");
        assert_eq!(result.contains("ENVELOPE"), envelope, "{result}");
        assert_eq!(result.contains(" BODY ("), body, "{result}");
        assert!(!result.contains("ATTACHMENT"), "BODY has extension fields: {result}");
    }
    assert!(command(&mut r, &mut w, "b", "FETCH 1 BODY").await?.contains("BODY (("));
    assert!(command(&mut r, &mut w, "b", "FETCH 1 (FAST UID)").await?.contains("BAD"));
    Ok(())
}

#[tokio::test]
async fn binary_decodes_sizes_partials_literal8_and_seen() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(&ms, &mds, MIME, "MIME", Flags(0), 1_700_000_000_000)?;
    let (mut r, mut w) = selected(ms, db).await?;
    let result = exchange(&mut r, &mut w, "b", b"b FETCH 1 (BINARY.PEEK[2] BINARY.SIZE[2] FLAGS)\r\n").await?;
    assert!(result.windows(b"BINARY[2] ~{4}\r\nA\0B\xff".len()).any(|s| s == b"BINARY[2] ~{4}\r\nA\0B\xff"), "{result:?}");
    assert!(String::from_utf8_lossy(&result).contains("BINARY.SIZE[2] 4 FLAGS ()"));
    let result = command(&mut r, &mut w, "b", "FETCH 1 (BINARY.PEEK[3.2] BINARY.SIZE[3.2])").await?;
    assert!(result.contains("{5}\r\nCafé BINARY.SIZE[3.2] 5"), "{result}");
    let result = exchange(&mut r, &mut w, "b", b"b FETCH 1 BINARY[2]<1.2>\r\n").await?;
    assert!(result.windows(b"~{2}\r\n\0B".len()).any(|s| s == b"~{2}\r\n\0B"), "{result:?}");
    assert!(String::from_utf8_lossy(&result).contains("FLAGS (\\Seen)"));
    Ok(())
}

#[tokio::test]
async fn binary_unknown_and_malformed_transfer_encoding_fail() -> Result<()> {
    for (encoding, data) in [("x-custom", "encoded"), ("base64", "a!=="), ("quoted-printable", "=GG")] {
        let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
        let raw = format!("Content-Type: application/octet-stream\r\nContent-Transfer-Encoding: {encoding}\r\n\r\n{data}");
        insert_email(&ms, &mds, raw.as_bytes(), "bad", Flags(0), 1_700_000_000_000)?;
        let (mut r, mut w) = selected(ms, db).await?;
        for atom in ["BINARY[1]", "BINARY.SIZE[1]"] {
            let result = command(&mut r, &mut w, "b", &format!("FETCH 1 {atom}")).await?;
            assert!(result.contains("b NO [UNKNOWN-CTE]"), "{encoding}: {result}");
        }
        assert!(command(&mut r, &mut w, "f", "FETCH 1 FLAGS").await?.contains("FLAGS ()"));
    }
    Ok(())
}

#[tokio::test]
async fn enable_rev2_esearch_save_and_dollar_survive_expunge() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    for n in 1..=3 { insert_email(&ms, &mds, b"Subject: x\r\n\r\nbody", &format!("m{n}"), Flags(0), 1_700_000_000_000)?; }
    let (client, _) = spawn_session(db, ms, ImapConfig::default()).await;
    let (mut r, mut w) = login(client).await?;
    let enabled = command(&mut r, &mut w, "e", "ENABLE IMAP4rev2 UTF8=ACCEPT CONDSTORE QRESYNC unknown").await?;
    assert!(enabled.starts_with("* ENABLED IMAP4rev2 UTF8=ACCEPT\r\n"), "{enabled}");
    command(&mut r, &mut w, "s", "SELECT INBOX").await?;
    let result = command(&mut r, &mut w, "q", "SEARCH ALL").await?;
    assert!(result.contains("* ESEARCH (TAG \"q\") ALL 1:3"), "{result}");
    let result = command(&mut r, &mut w, "q", "UID SEARCH RETURN (MIN MAX ALL COUNT SAVE) ALL").await?;
    assert!(result.contains("UID MIN 1 MAX 3 ALL 1:3 COUNT 3"), "{result}");
    command(&mut r, &mut w, "f", "UID STORE 1 +FLAGS (\\Deleted)").await?;
    command(&mut r, &mut w, "x", "UID EXPUNGE 1").await?;
    let result = command(&mut r, &mut w, "f", "FETCH $ UID").await?;
    assert!(result.contains("* 1 FETCH (UID 2)"), "{result}");
    assert!(result.contains("* 2 FETCH (UID 3)"), "{result}");
    let result = command(&mut r, &mut w, "q", "SEARCH RETURN (COUNT) $").await?;
    assert!(result.contains("COUNT 2"), "{result}");
    let result = command(&mut r, &mut w, "q", "SEARCH RETURN (SAVE) UID 999").await?;
    assert_eq!(result, "q OK SEARCH completed\r\n");
    assert_eq!(command(&mut r, &mut w, "f", "UID FETCH $ FLAGS").await?, "f OK UID FETCH completed\r\n");
    let result = command(&mut r, &mut w, "q", "SEARCH RETURN (MIN MAX ALL COUNT) UID 999").await?;
    assert!(result.contains("* ESEARCH (TAG \"q\") COUNT 0"), "{result}");
    Ok(())
}

#[tokio::test]
async fn legacy_search_and_save_min_max_with_uid_copy_and_store() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    for n in 1..=3 { insert_email(&ms, &mds, b"Subject: x\r\n\r\nbody", &format!("m{n}"), Flags(0), 1_700_000_000_000)?; }
    let (mut r, mut w) = selected(ms, db).await?;
    assert!(command(&mut r, &mut w, "q", "SEARCH ALL").await?.contains("* SEARCH 1 2 3"));
    command(&mut r, &mut w, "q", "SEARCH RETURN (MIN MAX SAVE) ALL").await?;
    let result = command(&mut r, &mut w, "f", "UID STORE $ +FLAGS (\\Flagged)").await?;
    assert_eq!(result.matches("FLAGS (\\Flagged)").count(), 2, "{result}");
    let result = command(&mut r, &mut w, "c", "UID COPY $ Archive").await?;
    assert!(result.contains("COPYUID"), "{result}");
    command(&mut r, &mut w, "s", "SELECT Archive").await?;
    assert_eq!(command(&mut r, &mut w, "f", "FETCH $ UID").await?, "f OK FETCH completed\r\n");
    Ok(())
}

#[tokio::test]
async fn status_aggregates_and_appendlimit_match_config() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let raw = b"Subject: x\r\n\r\nbody";
    insert_email(&ms, &mds, raw, "a", Flags(Flags::DELETED), 1_700_000_000_000)?;
    insert_email(&ms, &mds, raw, "b", Flags(0), 1_700_000_000_000)?;
    let cfg = ImapConfig { max_literal_bytes: 12345, ..ImapConfig::default() };
    let (client, _) = spawn_session(db, ms, cfg).await;
    let (mut r, mut w) = login(client).await?;
    let caps = command(&mut r, &mut w, "c", "CAPABILITY").await?;
    assert!(caps.contains("APPENDLIMIT=12345"), "{caps}");
    let result = command(&mut r, &mut w, "s", "STATUS INBOX (MESSAGES SIZE DELETED APPENDLIMIT RECENT)").await?;
    assert!(result.contains(&format!("MESSAGES 2 SIZE {} DELETED 1 APPENDLIMIT 12345 RECENT 0", raw.len()*2)), "{result}");
    assert!(command(&mut r, &mut w, "s", "STATUS INBOX (HIGHESTMODSEQ)").await?.contains("s BAD"));
    Ok(())
}

#[tokio::test]
async fn list_extended_references_patterns_status_and_special_use() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (mut r, mut w) = selected(ms, db).await?;
    command(&mut r, &mut w, "c", "CREATE Projects/One").await?;
    command(&mut r, &mut w, "c", "SUBSCRIBE Projects/One").await?;
    let result = command(&mut r, &mut w, "l", "LIST (SUBSCRIBED RECURSIVEMATCH) \"\" % RETURN (SUBSCRIBED CHILDREN)").await?;
    assert!(result.contains("\"Projects\" (CHILDINFO (\"SUBSCRIBED\"))"), "{result}");
    let result = command(&mut r, &mut w, "l", "LIST \"Projects/\" % RETURN (STATUS (MESSAGES SIZE APPENDLIMIT) CHILDREN)").await?;
    assert!(result.contains("\"Projects/One\""), "{result}");
    assert!(result.contains("* STATUS \"Projects/One\" (MESSAGES 0 SIZE 0 APPENDLIMIT"), "{result}");
    let result = command(&mut r, &mut w, "l", "LSUB \"Projects/\" *").await?;
    assert!(result.contains("\"Projects/One\""), "{result}");
    let result = command(&mut r, &mut w, "l", "LIST (REMOTE SPECIAL-USE) \"\" (INBOX Archive) RETURN (SPECIAL-USE)").await?;
    assert!(result.contains("\\Archive"), "{result}");
    assert!(result.contains("\\Inbox"), "{result}");
    assert!(!result.contains("Projects"), "{result}");
    assert!(command(&mut r, &mut w, "l", "LIST (RECURSIVEMATCH) \"\" *").await?.contains("l BAD"));
    assert!(command(&mut r, &mut w, "l", "LIST \"\" * RETURN (BOGUS)").await?.contains("l BAD"));
    Ok(())
}

#[tokio::test]
async fn utf8_mailbox_names_and_rev1_modified_utf7_roundtrip() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (client, _) = spawn_session(db, ms, ImapConfig::default()).await;
    let (mut r, mut w) = login(client).await?;
    assert!(command(&mut r, &mut w, "c", "CREATE \"Caf&AOk- &- tea\"").await?.contains("c OK"));
    let result = command(&mut r, &mut w, "l", "LIST \"\" *").await?;
    assert!(result.contains("Caf&AOk- &- tea"), "{result}");
    command(&mut r, &mut w, "e", "ENABLE UTF8=ACCEPT").await?;
    let result = command(&mut r, &mut w, "l", "LIST \"\" \"Café & tea\"").await?;
    assert!(result.contains("\"Café & tea\""), "{result}");
    assert!(command(&mut r, &mut w, "s", "SELECT \"Café & tea\"").await?.contains("s OK"));
    assert!(command(&mut r, &mut w, "c", "CREATE \"日本語\"").await?.contains("c OK"));
    Ok(())
}

#[tokio::test]
async fn general_literals_login_names_flags_search_and_multiappend() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (client, _) = spawn_session(db, ms, ImapConfig::default()).await;
    let (rr, mut w) = tokio::io::split(client);
    let mut r = BufReader::new(rr);
    read_line(&mut r).await?;
    let result = exchange(&mut r, &mut w, "a", b"a LOGIN {17+}\r\nalice@example.com {7+}\r\nhunter2\r\n").await?;
    assert!(String::from_utf8_lossy(&result).contains("a OK"));
    let result = exchange(&mut r, &mut w, "c", b"c CREATE {5+}\r\nBox A\r\n").await?;
    assert!(String::from_utf8_lossy(&result).contains("c OK"));
    assert!(String::from_utf8_lossy(&exchange(&mut r, &mut w, "n", b"n RENAME {5+}\r\nBox A {5+}\r\nBox B\r\n").await?).contains("n OK"));
    let one = b"Subject: One\r\n\r\nfirst";
    let two = b"Subject: Two\r\n\r\nsecond\0data";
    let mut wire = format!("a APPEND {{5+}}\r\nBox B (\\Seen project) \"01-Jan-2026 10:00:00 +0000\" {{{}+}}\r\n", one.len()).into_bytes();
    wire.extend_from_slice(one);
    wire.extend_from_slice(format!(" (\\Flagged) ~{{{}+}}\r\n", two.len()).as_bytes());
    wire.extend_from_slice(two); wire.extend_from_slice(b"\r\n");
    let result = exchange(&mut r, &mut w, "a", &wire).await?;
    assert!(String::from_utf8_lossy(&result).contains("APPENDUID"), "{result:?}");
    assert!(String::from_utf8_lossy(&result).contains(" 1:2]"), "{result:?}");
    command(&mut r, &mut w, "s", "SELECT \"Box B\"").await?;
    let result = exchange(&mut r, &mut w, "f", b"f STORE 2 +FLAGS ({6+}\r\nurgent)\r\n").await?;
    assert!(String::from_utf8_lossy(&result).contains("urgent"), "{result:?}");
    let result = exchange(&mut r, &mut w, "q", b"q SEARCH SUBJECT {3+}\r\nTwo\r\n").await?;
    assert!(String::from_utf8_lossy(&result).contains("* SEARCH 2"));
    command(&mut r, &mut w, "u", "UNSELECT").await?;
    assert!(String::from_utf8_lossy(&exchange(&mut r, &mut w, "d", b"d DELETE {5+}\r\nBox B\r\n").await?).contains("d OK"));
    Ok(())
}

#[tokio::test]
async fn multiappend_failure_leaves_no_partial_messages() -> Result<()> {
    let (_tmp, db, _mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    let (mut r, mut w) = selected(ms, db).await?;
    let one = b"Subject: One\r\n\r\nfirst";
    let mut wire = format!("a APPEND INBOX {{{}+}}\r\n", one.len()).into_bytes();
    wire.extend_from_slice(one); wire.extend_from_slice(b" {0+}\r\n\r\n");
    let result = exchange(&mut r, &mut w, "a", &wire).await?;
    assert!(String::from_utf8_lossy(&result).contains("a NO"), "{result:?}");
    assert!(command(&mut r, &mut w, "s", "STATUS INBOX (MESSAGES)").await?.contains("MESSAGES 0"));
    Ok(())
}

#[tokio::test]
async fn rename_inbox_preserves_identity_children_and_moves_messages() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(&ms, &mds, MIME, "message", Flags(Flags::FLAGGED), 1_700_000_000_000)?;
    let (mut r, mut w) = selected(ms, db).await?;
    command(&mut r, &mut w, "c", "CREATE INBOX/Child").await?;
    let before = command(&mut r, &mut w, "b", "STATUS INBOX (UIDVALIDITY UIDNEXT)").await?;
    assert!(command(&mut r, &mut w, "n", "RENAME INBOX Saved").await?.contains("n OK"));
    let after = command(&mut r, &mut w, "b", "STATUS INBOX (UIDVALIDITY UIDNEXT)").await?;
    assert_eq!(before, after);
    assert!(command(&mut r, &mut w, "b", "STATUS INBOX (MESSAGES)").await?.contains("MESSAGES 0"));
    assert!(command(&mut r, &mut w, "s", "SELECT INBOX/Child").await?.contains("s OK"));
    command(&mut r, &mut w, "s", "SELECT Saved").await?;
    assert!(command(&mut r, &mut w, "f", "FETCH 1 FLAGS").await?.contains("\\Flagged"));
    Ok(())
}

#[tokio::test]
async fn examine_fetch_and_close_unselect_have_distinct_effects() -> Result<()> {
    let (_tmp, db, mds, ms) = make_fixture("alice@example.com", "hunter2").await?;
    insert_email(&ms, &mds, b"Subject: x\r\n\r\nbody", "a", Flags(Flags::DELETED), 1_700_000_000_000)?;
    let (mut r, mut w) = selected(ms, db).await?;
    command(&mut r, &mut w, "s", "EXAMINE INBOX").await?;
    let result = command(&mut r, &mut w, "f", "FETCH 1 (BODY[] FLAGS)").await?;
    assert!(!result.contains("\\Seen"), "EXAMINE changed flags: {result}");
    command(&mut r, &mut w, "c", "CLOSE").await?;
    assert!(command(&mut r, &mut w, "s", "STATUS INBOX (MESSAGES)").await?.contains("MESSAGES 1"));
    command(&mut r, &mut w, "s", "SELECT INBOX").await?;
    command(&mut r, &mut w, "u", "UNSELECT").await?;
    assert!(command(&mut r, &mut w, "s", "STATUS INBOX (MESSAGES)").await?.contains("MESSAGES 1"));
    command(&mut r, &mut w, "s", "SELECT INBOX").await?;
    assert_eq!(command(&mut r, &mut w, "c", "CLOSE").await?, "c OK CLOSE completed\r\n");
    assert!(command(&mut r, &mut w, "s", "STATUS INBOX (MESSAGES)").await?.contains("MESSAGES 0"));
    Ok(())
}

