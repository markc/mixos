// SPDX-License-Identifier: MIT OR Apache-2.0

//! Real SMTP sessions, real local storage and the production delivery worker.
use anyhow::{Result, ensure};
use base64::Engine;
use bus::native_client::IncomingCommand;
use maild::{
    config::{Config, ListenSpec},
    mailstore::{ListOpts, MailStore, MailboxRole},
    runtime::{BuiltMaild, RuntimeOpts, build_runtime},
};
use mds::Mds;
use props::{Actor, DeleteOpts, MergeMode, RecordKey, SetOpts, Version, value::PropValue};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    time::timeout,
};

async fn fixture(
    tmp: &tempfile::TempDir,
    mx: Option<std::net::SocketAddr>,
) -> Result<(BuiltMaild, tokio_rustls::TlsConnector)> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
    let cert_path = tmp.path().join("cert.pem");
    let key_path = tmp.path().join("key.pem");
    std::fs::write(&cert_path, cert.cert.pem())?;
    std::fs::write(&key_path, cert.key_pair.serialize_pem())?;
    let cfg = Config {
        listen: "127.0.0.1:0".into(),
        base_url: "http://127.0.0.1".into(),
        hostname: "mail.example.com".into(),
        database_path: tmp.path().join("mail.db").to_string_lossy().into_owned(),
        blob_dir: tmp.path().join("blobs").to_string_lossy().into_owned(),
        mds_dir: tmp.path().join("mds").to_string_lossy().into_owned(),
        spam_db_dir: Some(tmp.path().join("spam").to_string_lossy().into_owned()),
        rule_stats_dir: Some(tmp.path().join("rules").to_string_lossy().into_owned()),
        smtp_inbound: Some(ListenSpec::Single("127.0.0.1:0".into())),
        smtp_smtps: Some(ListenSpec::Single("127.0.0.1:0".into())),
        imap_imaps: None,
        tls_cert: Some(cert_path.to_string_lossy().into_owned()),
        tls_key: Some(key_path.to_string_lossy().into_owned()),
        spam_enabled: Some(false),
        ..Config::default()
    };
    let opts = RuntimeOpts {
        enable_bus: false,
        disable_outbound_delivery: mx.is_none(),
        test_mx_overrides: mx
            .map(|addr| [("remote.example.net".into(), addr)].into())
            .unwrap_or_default(),
    };
    let built = build_runtime(&cfg, opts).await?;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.cert.der().clone())?;
    let client = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok((built, tokio_rustls::TlsConnector::from(Arc::new(client))))
}

fn opts(version: Version) -> SetOpts {
    SetOpts {
        expected_version: Some(version),
        merge: MergeMode::Replace,
        actor: Actor::service("smtp-test").unwrap(),
        cause: None,
        ts_ms: 0,
    }
}

async fn account(built: &BuiltMaild, email: &str) -> Result<i32> {
    let body = PropValue::Object(BTreeMap::from([
        ("email".into(), PropValue::String(email.into())),
        (
            "password".into(),
            PropValue::String(bcrypt::hash("test-password", 4)?),
        ),
        ("spam_enabled".into(), PropValue::Bool(false)),
    ]));
    built
        .accounts_runtime()
        .set(
            RecordKey::collection(maild::props::accounts::namespace_name(), email),
            body,
            opts(Version::zero()),
        )
        .await?;
    Ok(
        maild::db::account::get_by_email(&built.app_state.db.conn, email)
            .await?
            .unwrap()
            .id,
    )
}

async fn alias(
    built: &BuiltMaild,
    address: &str,
    targets: &[&str],
    version: Version,
) -> Result<Version> {
    let value = PropValue::Object(BTreeMap::from([
        ("address".into(), PropValue::String(address.into())),
        ("target".into(), PropValue::String(targets[0].into())),
        (
            "targets".into(),
            PropValue::List(
                targets
                    .iter()
                    .map(|s| PropValue::String((*s).into()))
                    .collect(),
            ),
        ),
        ("enabled".into(), PropValue::Bool(true)),
    ]));
    let rt = &built.app_state.aliases_runtime;
    rt.set(
        RecordKey::collection(maild::props::aliases::namespace_name(), address),
        value,
        opts(version),
    )
    .await?;
    Ok(rt
        .store()
        .get(&RecordKey::collection(
            maild::props::aliases::namespace_name(),
            address,
        ))
        .await?
        .value
        .version)
}

async fn reply<S: AsyncRead + Unpin>(io: &mut BufReader<S>) -> Result<String> {
    let mut response = String::new();
    loop {
        let mut line = String::new();
        ensure!(
            timeout(Duration::from_secs(10), io.read_line(&mut line)).await?? > 0,
            "SMTP EOF: {response}"
        );
        let more = line.as_bytes().get(3) == Some(&b'-');
        response.push_str(&line);
        if !more {
            return Ok(response);
        }
    }
}

async fn command<S: AsyncRead + AsyncWrite + Unpin>(
    io: &mut BufReader<S>,
    line: &str,
    code: &str,
) -> Result<String> {
    io.get_mut()
        .write_all(format!("{line}\r\n").as_bytes())
        .await?;
    let response = reply(io).await?;
    ensure!(response.starts_with(code), "{line}: {response}");
    Ok(response)
}

async fn send(built: &BuiltMaild, to: &str) -> Result<()> {
    let mut io = BufReader::new(TcpStream::connect(built.smtp_handle.inbound_addrs[0]).await?);
    reply(&mut io).await?;
    command(&mut io, "EHLO sender.example.org", "250").await?;
    command(
        &mut io,
        "MAIL FROM:<sender@example.org> SMTPUTF8 BODY=8BITMIME",
        "250",
    )
    .await?;
    command(&mut io, &format!("RCPT TO:<{to}>"), "250").await?;
    command(&mut io, "DATA", "354").await?;
    command(&mut io,"From: sender@example.org\r\nTo: recipient@example.com\r\nSubject: café\r\n\r\nUTF-8 body ✓\r\n.","250").await?;
    Ok(())
}

fn inbox_count(built: &BuiltMaild, id: i32) -> Result<usize> {
    let ms = &built.app_state.mailstore;
    let inbox = ms.mailbox_by_role(id, MailboxRole::Inbox)?.unwrap();
    Ok(ms
        .list_emails_in_mailbox(id, inbox, ListOpts::default())?
        .len())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn utf8_inbound_and_authenticated_submission_store_mail() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (built, tls) = fixture(&tmp, None).await?;
    let address = "élise@例子.example.com";
    let id = account(&built, address).await?;
    account(&built, "alice@example.com").await?;
    // A hosted IDN is configured using its DNS A-label. The envelope and
    // mailbox retain the U-label and must pass the authoritative RCPT gate.
    for domain in ["xn--fsqu00a.example.com", "example.com"] {
        built
            .app_state
            .domains_runtime
            .set(
                RecordKey::collection(maild::props::domains::namespace_name(), domain),
                maild::props::domains::defaults_value(domain),
                opts(Version::zero()),
            )
            .await?;
    }
    let mut io = BufReader::new(TcpStream::connect(built.smtp_handle.inbound_addrs[0]).await?);
    reply(&mut io).await?;
    let ehlo = command(&mut io, "EHLO sender.example.org", "250").await?;
    assert!(ehlo.contains("SMTPUTF8") && ehlo.contains("8BITMIME"));
    command(&mut io, "MAIL FROM:<élise@example.org>", "553").await?;
    command(
        &mut io,
        "MAIL FROM:<sender@example.org> SMTPUTF8=yes",
        "555",
    )
    .await?;
    for parameter in ["AUTH=<>", "AUTH=sender+2Btag@example.org"] {
        command(
            &mut io,
            &format!("MAIL FROM:<sender@example.org> {parameter}"),
            "250",
        )
        .await?;
        command(&mut io, "RSET", "250").await?;
    }
    for parameter in ["AUTH=", "AUTH=not-an-address", "AUTH=sender+ZZ@example.org"] {
        command(
            &mut io,
            &format!("MAIL FROM:<sender@example.org> {parameter}"),
            "555",
        )
        .await?;
    }
    command(&mut io, "MAIL FROM:<sender@example.org>", "250").await?;
    command(&mut io, &format!("RCPT TO:<{address}>"), "553").await?;
    command(&mut io, "RCPT TO:<alice@example.com>", "250").await?;
    command(&mut io, "DATA", "354").await?;
    command(
        &mut io,
        "From: sender@example.org\r\nSubject: café\r\n\r\nbody\r\n.",
        "250",
    )
    .await?;
    command(
        &mut io,
        "MAIL FROM:<sender@example.org> BODY=8BITMIME",
        "250",
    )
    .await?;
    command(&mut io, "RCPT TO:<alice@example.com>", "250").await?;
    command(&mut io, "DATA", "354").await?;
    command(&mut io, "MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\nContent-Disposition: inline; filename=\"café.txt\"\r\n\r\nbody\r\n--x--\r\n.", "250").await?;
    send(&built, address).await?;
    assert_eq!(inbox_count(&built, id)?, 1);
    let ms = &built.app_state.mailstore;
    let inbox = ms.mailbox_by_role(id, MailboxRole::Inbox)?.unwrap();
    let stored = ms.list_emails_in_mailbox(id, inbox, ListOpts::default())?;
    let bytes = ms.mds().get_blob(&stored[0].blob_hash)?;
    assert!(String::from_utf8_lossy(&bytes).contains("Subject: café"));
    assert!(String::from_utf8_lossy(&bytes).contains("UTF-8 body ✓"));
    let (rc, body) = maild::bus::accounts::dispatch(
        "lookup",
        &bus_cmd("maild.accounts.lookup", json!({"email":address})),
        &built.app_state.db,
        &built.app_state.mailstore,
        &built.accounts_runtime(),
    )
    .await;
    ensure!(rc == 0, "stats: {body}");
    assert_eq!(serde_json::from_str::<Value>(&body)?["account_id"], id);
    // A legacy row has no MDS account set; numeric inspection must still work.
    built.app_state.db.conn.lock().unwrap().execute(
        "INSERT INTO accounts(id,email,password) VALUES(100,'legacy@example.com','test-hash')",
        [],
    )?;
    let (rc, body) = maild::bus::accounts::dispatch(
        "lookup",
        &bus_cmd(
            "maild.accounts.lookup",
            json!({"email":"legacy@example.com"}),
        ),
        &built.app_state.db,
        &built.app_state.mailstore,
        &built.accounts_runtime(),
    )
    .await;
    ensure!(rc == 0, "legacy identity: {body}");
    assert_eq!(serde_json::from_str::<Value>(&body)?["account_id"], 100);
    let stream = tls
        .connect(
            rustls::pki_types::ServerName::try_from("localhost")?,
            TcpStream::connect(built.smtp_handle.smtps_addrs[0]).await?,
        )
        .await?;
    let mut io = BufReader::new(stream);
    reply(&mut io).await?;
    assert!(
        command(&mut io, "EHLO localhost", "250")
            .await?
            .contains("SMTPUTF8")
    );
    let auth =
        base64::engine::general_purpose::STANDARD.encode(format!("\0{address}\0test-password"));
    command(&mut io, &format!("AUTH PLAIN {auth}"), "235").await?;
    command(
        &mut io,
        &format!("MAIL FROM:<{address}> AUTH=<> SMTPUTF8 BODY=8BITMIME"),
        "250",
    )
    .await?;
    command(&mut io, &format!("RCPT TO:<{address}>"), "250").await?;
    command(&mut io, "DATA", "354").await?;
    command(
        &mut io,
        &format!("From: {address}\r\nTo: {address}\r\nSubject: UTF-8 ✓\r\n\r\ncafé\r\n."),
        "250",
    )
    .await?;
    assert_eq!(inbox_count(&built, id)?, 2);
    command(
        &mut io,
        "MAIL FROM:<élise@例子.example.com> SMTPUTF8",
        "250",
    )
    .await?;
    command(&mut io, "RCPT TO:<alice@example.com>", "250").await?;
    command(&mut io, "RSET", "250").await?;
    command(
        &mut io,
        "MAIL FROM:<élise@xn--fsqu00a.example.com> SMTPUTF8",
        "250",
    )
    .await?;
    command(&mut io, "RSET", "250").await?;
    command(&mut io, "MAIL FROM:<élise@例子.example.com>", "553").await?;
    send(&built, "élise@xn--fsqu00a.example.com").await?;
    assert_eq!(inbox_count(&built, id)?, 3);
    let ascii_domain = account(&built, "marie@xn--fsqu00a.example.com").await?;
    send(&built, "marie@例子.example.com").await?;
    assert_eq!(inbox_count(&built, ascii_domain)?, 1);
    alias(
        &built,
        "postmaster@例子.example.com",
        &["élise@xn--fsqu00a.example.com"],
        Version::zero(),
    )
    .await?;
    send(&built, "postmaster@xn--fsqu00a.example.com").await?;
    assert_eq!(inbox_count(&built, id)?, 4);
    assert!(
        maild::props::aliases::sender_authorized(
            &built.app_state.aliases_runtime,
            "postmaster@xn--fsqu00a.example.com",
            address
        )
        .await
    );
    let quoted = account(&built, "\"élise\"@example.com").await?;
    send(&built, "\"élise\"@example.com").await?;
    assert_eq!(inbox_count(&built, quoted)?, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aliases_resurrect_repoint_fanout_and_mailbox_precedence_on_wire() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (built, _) = fixture(&tmp, None).await?;
    let alice = account(&built, "alice@example.com").await?;
    let bob = account(&built, "bob@example.com").await?;
    let version = alias(
        &built,
        "sales@example.com",
        &["alice@example.com"],
        Version::zero(),
    )
    .await?;
    let rt = &built.app_state.aliases_runtime;
    let key = RecordKey::collection(maild::props::aliases::namespace_name(), "sales@example.com");
    rt.delete(
        key.clone(),
        DeleteOpts {
            expected_version: Some(version),
            actor: Actor::service("smtp-test")?,
            cause: None,
            ts_ms: 0,
        },
    )
    .await?;
    assert!(
        alias(
            &built,
            "sales@example.com",
            &["alice@example.com"],
            Version::zero()
        )
        .await
        .is_err()
    );
    let anchor = rt.store().version_anchor(&key).await?.unwrap();
    let version = alias(&built, "sales@example.com", &["alice@example.com"], anchor).await?;
    send(&built, "sales@example.com").await?;
    alias(&built, "sales@example.com", &["bob@example.com"], version).await?;
    send(&built, "sales@example.com").await?;
    assert_eq!(
        (inbox_count(&built, alice)?, inbox_count(&built, bob)?),
        (1, 1)
    );
    alias(
        &built,
        "@example.com",
        &["alice@example.com", "bob@example.com"],
        Version::zero(),
    )
    .await?;
    send(&built, "missing@example.com").await?;
    send(&built, "alice@example.com").await?;
    send(&built, "sales@example.com").await?;
    assert_eq!(
        (inbox_count(&built, alice)?, inbox_count(&built, bob)?),
        (3, 3)
    );
    assert!(
        alias(
            &built,
            "outside@example.com",
            &["remote@example.net"],
            Version::zero()
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("off-site forwarding")
    );
    assert!(
        !maild::props::aliases::sender_authorized(rt, "missing@example.com", "alice@example.com")
            .await
    );
    alias(
        &built,
        "team@example.com",
        &["alice@example.com", "bob@example.com"],
        Version::zero(),
    )
    .await?;
    assert!(
        maild::props::aliases::sender_authorized(rt, "team@example.com", "alice@example.com").await
    );
    assert!(
        maild::props::aliases::sender_authorized(rt, "team@example.com", "bob@example.com").await
    );
    let identities = maild::jmap::submission::identity_get(
        &built.app_state.db,
        built.app_state.domains_runtime.clone(),
        rt,
        bob,
        json!({}),
    )
    .await?;
    let identities = identities["list"].as_array().unwrap();
    assert!(
        identities
            .iter()
            .any(|identity| identity["email"] == "team@example.com")
    );
    assert!(
        !identities
            .iter()
            .any(|identity| identity["email"] == "@example.com")
    );
    let submission = bus_cmd(
        "maild.submit",
        json!({"operation_id":"aliases-local-1","account":"alice@example.com",
        "to":["team@example.com","bob@example.com"],"subject":"fanout","text":"local fanout"}),
    );
    let (rc, body) = maild::bus::submission::dispatch(
        &submission,
        &built.app_state.db,
        &built.app_state.mailstore,
        rt,
        1024 * 1024,
    )
    .await;
    ensure!(rc == 0, "local fanout: {body}");
    let receipt: Value = serde_json::from_str(&body)?;
    let query = bus_cmd(
        "maild.submission.status",
        json!({"id":receipt["submission_id"]}),
    );
    let (rc, body) = maild::bus::submission::status(&query, &built.app_state.db).await;
    ensure!(rc == 0, "local status: {body}");
    let status: Value = serde_json::from_str(&body)?;
    assert_eq!(status["delivery_confirmed"], true);
    assert_eq!(status["recipients"].as_array().unwrap().len(), 3);
    assert_eq!(
        (inbox_count(&built, alice)?, inbox_count(&built, bob)?),
        (4, 4)
    );
    Ok(())
}

fn bus_cmd(command: &str, body: Value) -> IncomingCommand {
    IncomingCommand {
        generation: 0,
        command: command.into(),
        from: "smtp-test".into(),
        id: None,
        args: Value::Null,
        headers: BTreeMap::new(),
        body: body.to_string(),
    }
}

async fn submit_native(built: &BuiltMaild, body: Value) -> Result<uuid::Uuid> {
    let (rc, body) = maild::bus::submission::dispatch(
        &bus_cmd("maild.submit", body),
        &built.app_state.db,
        &built.app_state.mailstore,
        &built.app_state.aliases_runtime,
        1024 * 1024,
    )
    .await;
    ensure!(rc == 0, "submit: {body}");
    Ok(serde_json::from_str::<Value>(&body)?["submission_id"]
        .as_str()
        .unwrap()
        .parse()?)
}

async fn status(built: &BuiltMaild, id: uuid::Uuid) -> Result<Value> {
    let (rc, body) = maild::bus::submission::status(
        &bus_cmd("maild.submission.status", json!({"id":id})),
        &built.app_state.db,
    )
    .await;
    ensure!(rc == 0, "status: {body}");
    Ok(serde_json::from_str(&body)?)
}

async fn wait_delivered(built: &BuiltMaild, id: uuid::Uuid) -> Result<Value> {
    timeout(Duration::from_secs(65), async {
        loop {
            let result = status(built, id).await?;
            if result["delivery_confirmed"] == true
                && maild::smtp::queue::list(&built.app_state.db.conn, 10)
                    .await?
                    .is_empty()
            {
                return Ok(result);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_queue_ids_migrate_and_second_message_transmits_without_retagging_first()
-> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let peer_addr = listener.local_addr()?;
    let peer = tokio::spawn(async move {
        let mut messages = Vec::new();
        for _ in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut io = BufReader::new(stream);
            io.get_mut()
                .write_all(b"220 mx.example.net\r\n")
                .await
                .unwrap();
            let mut data = false;
            let mut message = String::new();
            loop {
                let mut line = String::new();
                if io.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let reply = if data {
                    if line == ".\r\n" {
                        data = false;
                        "250 2.0.0 stored\r\n"
                    } else {
                        message.push_str(&line);
                        continue;
                    }
                } else if line.starts_with("EHLO") {
                    "250-mx.example.net\r\n250 8BITMIME\r\n"
                } else if line.starts_with("DATA") {
                    data = true;
                    "354 send\r\n"
                } else if line.starts_with("QUIT") {
                    break;
                } else {
                    "250 ok\r\n"
                };
                io.get_mut().write_all(reply.as_bytes()).await.unwrap();
            }
            messages.push(message);
        }
        messages
    });
    let tmp = tempfile::tempdir()?;
    let (built, _) = fixture(&tmp, Some(peer_addr)).await?;
    account(&built, "alice@example.com").await?;
    {
        let conn = built.app_state.db.conn.lock().unwrap();
        let sql: String = conn.query_row(
            "SELECT sql FROM sqlite_master WHERE name='smtp_queue'",
            [],
            |r| r.get(0),
        )?;
        conn.execute_batch("DROP TABLE smtp_queue")?;
        conn.execute_batch(&sql.replace("AUTOINCREMENT", ""))?;
    }
    let first = submit_native(&built,json!({"operation_id":"reuse-1","account":"alice@example.com","to":["same@remote.example.net"],"subject":"M1","text":"first payload"})).await?;
    let first_queue_id: i64 = built.app_state.db.conn.lock().unwrap().query_row(
        "SELECT queue_id FROM smtp_delivery WHERE submission_id=?1",
        [first.to_string()],
        |r| r.get(0),
    )?;
    let original = wait_delivered(&built, first).await?;
    // Upgrade the legacy database after its last queue row has disappeared.
    built.app_state.db.migrate().await?;
    built.app_state.db.migrate().await?;
    let second = submit_native(&built,json!({"operation_id":"reuse-2","account":"alice@example.com","to":["same@remote.example.net"],"subject":"M2","text":"second payload"})).await?;
    let second_queue_id: i64 = built.app_state.db.conn.lock().unwrap().query_row(
        "SELECT queue_id FROM smtp_delivery WHERE submission_id=?1",
        [second.to_string()],
        |r| r.get(0),
    )?;
    assert!(second_queue_id > first_queue_id);
    wait_delivered(&built, second).await?;
    assert_eq!(status(&built, first).await?, original);
    let messages = timeout(Duration::from_secs(5), peer).await??;
    assert!(messages[0].contains("first payload"));
    assert!(messages[1].contains("second payload"));
    Ok(())
}

async fn authenticated(
    built: &BuiltMaild,
    tls: &tokio_rustls::TlsConnector,
    email: &str,
) -> Result<BufReader<tokio_rustls::client::TlsStream<TcpStream>>> {
    let stream = tls
        .connect(
            rustls::pki_types::ServerName::try_from("localhost")?,
            TcpStream::connect(built.smtp_handle.smtps_addrs[0]).await?,
        )
        .await?;
    let mut io = BufReader::new(stream);
    reply(&mut io).await?;
    command(&mut io, "EHLO client.example.org", "250").await?;
    let auth =
        base64::engine::general_purpose::STANDARD.encode(format!("\0{email}\0test-password"));
    command(&mut io, &format!("AUTH PLAIN {auth}"), "235").await?;
    Ok(io)
}

async fn raw_header_mail<S: AsyncRead + AsyncWrite + Unpin>(io: &mut BufReader<S>) -> Result<()> {
    command(io, "MAIL FROM:<alice@example.com> BODY=8BITMIME", "250").await?;
    command(io, "RCPT TO:<alice@example.com>", "250").await?;
    command(io, "DATA", "354").await?;
    command(
        io,
        "From: alice@example.com\r\nSubject: café\r\n\r\nASCII body\r\n.",
        "250",
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_raw_headers_are_accepted_on_inbound_and_submission() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (built, tls) = fixture(&tmp, None).await?;
    let alice = account(&built, "alice@example.com").await?;
    let mut io = BufReader::new(TcpStream::connect(built.smtp_handle.inbound_addrs[0]).await?);
    reply(&mut io).await?;
    command(&mut io, "EHLO client.example.org", "250").await?;
    raw_header_mail(&mut io).await?;
    let mut io = authenticated(&built, &tls, "alice@example.com").await?;
    raw_header_mail(&mut io).await?;
    assert_eq!(inbox_count(&built, alice)?, 2);
    let ms = &built.app_state.mailstore;
    for message in ms.list_emails_in_mailbox(
        alice,
        ms.mailbox_by_role(alice, MailboxRole::Inbox)?.unwrap(),
        ListOpts::default(),
    )? {
        assert!(
            String::from_utf8(ms.mds().get_blob(&message.blob_hash)?)?.contains("Subject: café")
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejected_mail_preserves_accepted_transaction_in_both_protocols() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (built, tls) = fixture(&tmp, None).await?;
    let alice = account(&built, "alice@example.com").await?;
    let mut io = BufReader::new(TcpStream::connect(built.smtp_handle.inbound_addrs[0]).await?);
    reply(&mut io).await?;
    command(&mut io, "EHLO client.example.org", "250").await?;
    command(&mut io, "MAIL FROM:<élise@example.org> SMTPUTF8", "250").await?;
    command(&mut io, "RCPT TO:<alice@example.com>", "250").await?;
    command(&mut io, "MAIL FROM:<bad>", "501").await?;
    command(&mut io, "MAIL FROM:<other@example.org> SMTPUTF8=yes", "555").await?;
    command(&mut io, "MAIL FROM:<élise@example.org>", "553").await?;
    command(&mut io, "DATA", "354").await?;
    command(
        &mut io,
        "From: élise@example.org\r\nSubject: preserved\r\n\r\nfirst\r\n.",
        "250",
    )
    .await?;
    let mut io = authenticated(&built, &tls, "alice@example.com").await?;
    command(&mut io, "MAIL FROM:<alice@example.com>", "250").await?;
    command(&mut io, "RCPT TO:<alice@example.com>", "250").await?;
    command(&mut io, "MAIL FROM:<other@example.org>", "530").await?;
    command(&mut io, "DATA", "354").await?;
    command(
        &mut io,
        "From: alice@example.com\r\nSubject: preserved\r\n\r\nsecond\r\n.",
        "250",
    )
    .await?;
    assert_eq!(inbox_count(&built, alice)?, 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn address_literals_reach_delivery_policy_after_syntax_validation() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (built, tls) = fixture(&tmp, None).await?;
    account(&built, "alice@example.com").await?;
    let mut io = BufReader::new(TcpStream::connect(built.smtp_handle.inbound_addrs[0]).await?);
    reply(&mut io).await?;
    command(&mut io, "EHLO client.example.org", "250").await?;
    for domain in ["[192.0.2.1]", "[IPv6:2001:db8::1]"] {
        command(&mut io, &format!("MAIL FROM:<user@{domain}>"), "250").await?;
        command(&mut io, &format!("RCPT TO:<user@{domain}>"), "550").await?;
        command(&mut io, "RSET", "250").await?;
    }
    for domain in ["[999.0.2.1]", "[IPv6:invalid]", "[2001:db8::1]"] {
        command(&mut io, &format!("MAIL FROM:<user@{domain}>"), "501").await?;
    }
    let mut io = authenticated(&built, &tls, "alice@example.com").await?;
    for domain in ["[192.0.2.1]", "[IPv6:2001:db8::1]"] {
        command(&mut io, "MAIL FROM:<alice@example.com>", "250").await?;
        command(&mut io, &format!("RCPT TO:<user@{domain}>"), "250").await?;
        command(&mut io, "RSET", "250").await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idn_alias_spelling_cannot_be_shadowed_by_account_creation() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let (built, _) = fixture(&tmp, None).await?;
    account(&built, "alice@example.com").await?;
    for (alias_addr, mailbox) in [
        ("team@xn--fsqu00a.example.com", "team@例子.example.com"),
        ("staff@例子.example.com", "staff@xn--fsqu00a.example.com"),
    ] {
        alias(&built, alias_addr, &["alice@example.com"], Version::zero()).await?;
        let error = account(&built, mailbox).await.unwrap_err().to_string();
        assert!(error.contains("already a maild.aliases alias"), "{error}");
        assert!(
            maild::db::account::get_by_email(&built.app_state.db.conn, mailbox)
                .await?
                .is_none()
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_fanout_defers_only_failed_targets_and_retries_without_duplicates() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let tmp = tempfile::tempdir()?;
    let (built, _) = fixture(&tmp, Some(listener.local_addr()?)).await?;
    let alice = account(&built, "alice@example.com").await?;
    let bob = account(&built, "bob@example.com").await?;
    alias(
        &built,
        "team@example.com",
        &["alice@example.com", "bob@example.com"],
        Version::zero(),
    )
    .await?;
    let ms = &built.app_state.mailstore;
    let bob_inbox = ms.mailbox_by_role(bob, MailboxRole::Inbox)?.unwrap();
    let first = submit_native(&built,json!({"operation_id":"local-seed","account":"alice@example.com","to":["alice@example.com"],"subject":"seed","text":"seed"})).await?;
    let email_id: String = built.app_state.db.conn.lock().unwrap().query_row(
        "SELECT email_id FROM email_submissions WHERE id=?1",
        [first.to_string()],
        |r| r.get(0),
    )?;
    for native in [true, false] {
        ms.update_mailbox(bob, bob_inbox, None, None, Some(None), None, None)?;
        let before_alice = inbox_count(&built, alice)?;
        let id = if native {
            submit_native(&built,json!({"operation_id":"local-partial","account":"alice@example.com","to":["team@example.com","bob@example.com"],"subject":"partial","text":"fanout"})).await?
        } else {
            let created = maild::jmap::submission::set(&built.app_state.db,ms,&built.app_state.aliases_runtime,alice,
                json!({"create":{"x":{"emailId":email_id,"envelope":{"mailFrom":{"email":"alice@example.com"},"rcptTo":[{"email":"team@example.com"},{"email":"bob@example.com"}]}}}})).await?;
            created["created"]["x"]["id"].as_str().unwrap().parse()?
        };
        let partial = status(&built, id).await?;
        let results = partial["recipients"].as_array().unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(
            results
                .iter()
                .filter(|r| r["status"] == "deferred" && r["target"] == "bob@example.com")
                .count(),
            2
        );
        assert!(
            results
                .iter()
                .any(|r| r["status"] == "delivered" && r["target"] == "alice@example.com")
        );
        assert_eq!(inbox_count(&built, alice)?, before_alice + 1);
        let queued = maild::smtp::queue::list(&built.app_state.db.conn, 10).await?;
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].local_account_id, Some(bob));
        assert_eq!(
            queued[0].attempts, 1,
            "the initial failed local attempt must be counted"
        );
        ms.update_mailbox(
            bob,
            bob_inbox,
            None,
            None,
            Some(Some(MailboxRole::Inbox)),
            None,
            None,
        )?;
        maild::smtp::queue::flush(&built.app_state.db.conn).await?;
        wait_delivered(&built, id).await?;
        assert_eq!(inbox_count(&built, alice)?, before_alice + 1);
        assert_eq!(inbox_count(&built, bob)?, if native { 1 } else { 2 });
        if native {
            // Recreate the durable state at a crash between the delivered receipt
            // commit and queue removal, after a real successful worker delivery.
            wait_queue_empty(&built).await?;
            built.app_state.db.conn.lock().unwrap().execute(
                "INSERT INTO smtp_queue(id,from_addr,to_addrs,blob_hash,local_account_id,attempts) VALUES(?1,?2,?3,?4,?5,?6)",
                rusqlite::params![queued[0].id,queued[0].from_addr,serde_json::to_string(&queued[0].to_addrs)?,mds::blob::hex(&queued[0].blob_hash.unwrap()),bob,queued[0].attempts],
            )?;
            wait_queue_empty(&built).await?;
            assert_eq!(inbox_count(&built, bob)?, 1, "a terminal local receipt must not be delivered twice after recovery");
            assert_eq!(inbox_count(&built, alice)?, before_alice + 1);
        }
    }
    // Deleting an account must not deliver a retry into its orphaned MDS set.
    ms.update_mailbox(bob, bob_inbox, None, None, Some(None), None, None)?;
    let deleted = submit_native(&built,json!({"operation_id":"local-deleted","account":"alice@example.com","to":["team@example.com"],"subject":"deleted target","text":"fanout"})).await?;
    ms.update_mailbox(
        bob,
        bob_inbox,
        None,
        None,
        Some(Some(MailboxRole::Inbox)),
        None,
        None,
    )?;
    built
        .app_state
        .db
        .conn
        .lock()
        .unwrap()
        .execute("DELETE FROM accounts WHERE id=?1", [bob])?;
    maild::smtp::queue::flush(&built.app_state.db.conn).await?;
    timeout(Duration::from_secs(65), async {
        loop {
            let results = status(&built, deleted).await?;
            if results["recipients"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["target"] == "bob@example.com" && r["status"] == "bounced")
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    assert_eq!(inbox_count(&built, bob)?, 2);
    Ok(())
}

async fn wait_queue_empty(built: &BuiltMaild) -> Result<()> {
    timeout(Duration::from_secs(65), async {
        loop {
            if maild::smtp::queue::list(&built.app_state.db.conn, 10).await?.is_empty() {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dangling_alias_targets_do_not_block_inbound_submission_or_bounces() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let peer_addr = listener.local_addr()?;
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut io = BufReader::new(stream);
        io.get_mut()
            .write_all(b"220 mx.example.net\r\n")
            .await
            .unwrap();
        let mut line = String::new();
        io.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("EHLO"));
        io.get_mut()
            .write_all(b"250-mx.example.net\r\n250 8BITMIME\r\n")
            .await
            .unwrap();
        line.clear();
        assert_eq!(io.read_line(&mut line).await.unwrap(), 0);
    });
    let tmp = tempfile::tempdir()?;
    let (built, _) = fixture(&tmp, Some(peer_addr)).await?;
    let alice = account(&built, "alice@example.com").await?;
    let bob = account(&built, "bob@example.com").await?;
    for address in ["tëam@example.com", "@example.com"] {
        alias(
            &built,
            address,
            &["alice@example.com", "bob@example.com"],
            Version::zero(),
        )
        .await?;
    }
    built
        .app_state
        .db
        .conn
        .lock()
        .unwrap()
        .execute("DELETE FROM accounts WHERE id=?1", [bob])?;
    send(&built, "tëam@example.com").await?;
    send(&built, "missing@example.com").await?;
    let id = submit_native(&built,json!({"operation_id":"dangling-local","account":"alice@example.com","to":["tëam@example.com","unknown@example.com"],"subject":"local","text":"body"})).await?;
    assert_eq!(status(&built, id).await?["delivery_confirmed"], true);
    assert_eq!(inbox_count(&built, alice)?, 3);
    let bounce = submit_native(&built,json!({"operation_id":"dangling-bounce","account":"alice@example.com","from":"tëam@example.com","to":["x@remote.example.net"],"subject":"café","text":"body"})).await?;
    timeout(Duration::from_secs(65), peer).await??;
    timeout(Duration::from_secs(10), async {
        loop {
            if status(&built, bounce).await?["recipients"][0]["status"] == "bounced"
                && inbox_count(&built, alice)? == 4
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    built
        .app_state
        .db
        .conn
        .lock()
        .unwrap()
        .execute("DELETE FROM accounts WHERE id=?1", [alice])?;
    let error = maild::props::aliases::recipient_accounts(
        &built.app_state.aliases_runtime,
        &built.app_state.db.conn,
        "tëam@example.com",
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("no remaining local targets"), "{error}");
    assert!(
        maild::smtp::queue::list(&built.app_state.db.conn, 10)
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receipt_retention_expires_terminal_submissions_and_cleans_unlinked_outcomes() -> Result<()>
{
    let tmp = tempfile::tempdir()?;
    let (built, _) = fixture(&tmp, None).await?;
    let alice = account(&built, "alice@example.com").await?;
    assert_eq!(
        maild::smtp::SmtpConfig::default().delivery_retention_days,
        30
    );
    let configured: Config = config::from_conf_mix_str("delivery_retention_days: 7\n")?;
    assert_eq!(configured.delivery_retention_days, Some(7));
    let mut ids = Vec::new();
    for operation in ["expired", "recent", "pending", "mixed"] {
        ids.push(submit_native(&built,json!({"operation_id":operation,"account":"alice@example.com",
            "to":if operation=="mixed" {vec!["x@remote.example.net","later@remote.example.net"]} else {vec!["x@remote.example.net"]},"subject":operation,"text":"body"})).await?);
    }
    let entries = maild::smtp::queue::list(&built.app_state.db.conn, 10).await?;
    let hash = entries[0].blob_hash.unwrap();
    for entry in &entries[..2] {
        maild::smtp::queue::recipient_result(
            &built.app_state.db.conn,
            entry.id,
            "x@remote.example.net",
            "delivered",
            "250 stored",
        )
        .await?;
        maild::smtp::queue::mark_delivered(&built.app_state.db.conn, entry.id).await?;
    }
    maild::smtp::queue::recipient_result(
        &built.app_state.db.conn,
        entries[3].id,
        "x@remote.example.net",
        "delivered",
        "250 stored",
    )
    .await?;
    {
        let conn = built.app_state.db.conn.lock().unwrap();
        conn.execute(
            "UPDATE email_submissions SET created_at=datetime('now','-31 days')",
            [],
        )?;
        conn.execute("UPDATE smtp_delivery SET updated_at=datetime('now','-31 days') WHERE submission_id<>?1",[ids[1].to_string()])?;
        conn.execute(
            "UPDATE smtp_delivery SET updated_at=datetime('now','-29 days') WHERE submission_id=?1",
            [ids[1].to_string()],
        )?;
    }
    for permanent in [false, true] {
        let id = maild::smtp::queue::enqueue(
            &built.app_state.db.conn,
            "alice@example.com",
            &["x@remote.example.net".into()],
            hash,
        )
        .await?;
        if permanent {
            maild::smtp::queue::mark_permanent_failure(&built.app_state.db.conn, id, "550 refused")
                .await?;
        } else {
            maild::smtp::queue::mark_delivered(&built.app_state.db.conn, id).await?;
        }
        let count: i64 = built.app_state.db.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM smtp_delivery WHERE queue_id=?1",
            [id],
            |r| r.get(0),
        )?;
        assert_eq!(count, 0, "unlinked SMTP receipt outlived its queue payload");
    }
    let before =
        maild::db::changelog::current_state(&built.app_state.db.conn, alice, "EmailSubmission")
            .await?;
    maild::smtp::queue::sweep_receipts(&built.app_state.db.conn, 30).await?;
    let changes =
        maild::jmap::submission::changes(&built.app_state.db, alice, json!({"sinceState":before}))
            .await?;
    assert_eq!(changes["destroyed"], json!([ids[0]]));
    assert_ne!(changes["newState"], json!(before));
    let (rc, _) = maild::bus::submission::status(
        &bus_cmd("maild.submission.status", json!({"id":ids[0]})),
        &built.app_state.db,
    )
    .await;
    assert_eq!(rc, 10);
    assert_eq!(status(&built, ids[1]).await?["delivery_confirmed"], true);
    assert_eq!(
        status(&built, ids[2]).await?["recipients"][0]["status"],
        "queued"
    );
    assert_eq!(
        status(&built, ids[3]).await?["recipients"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    maild::smtp::queue::sweep_receipts(&built.app_state.db.conn, 7).await?;
    let count: i64 = built.app_state.db.conn.lock().unwrap().query_row(
        "SELECT COUNT(*) FROM email_submissions WHERE id=?1",
        [ids[1].to_string()],
        |r| r.get(0),
    )?;
    assert_eq!(count, 0);
    assert_eq!(
        status(&built, ids[2]).await?["recipients"][0]["status"],
        "queued"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_queue_and_receipt_schema_upgrade_preserves_pending_payload_and_status() -> Result<()>
{
    let tmp = tempfile::tempdir()?;
    let (built, _) = fixture(&tmp, None).await?;
    account(&built, "alice@example.com").await?;
    let id = submit_native(&built,json!({"operation_id":"upgrade-pending","account":"alice@example.com","to":["x@remote.example.net"],"subject":"migration","text":"pending payload"})).await?;
    let conn = &built.app_state.db.conn;
    let queue_id = maild::smtp::queue::list(conn, 10).await?[0].id;
    maild::smtp::queue::mark_failed(conn, queue_id, "451 stored diagnostic").await?;
    let original = status(&built, id).await?;
    let hash = maild::smtp::queue::list(conn, 10).await?[0].blob_hash;
    {
        let conn = conn.lock().unwrap();
        let sql: String = conn.query_row(
            "SELECT sql FROM sqlite_master WHERE name='smtp_queue'",
            [],
            |r| r.get(0),
        )?;
        let legacy = sql.replace("AUTOINCREMENT", "");
        conn.execute_batch("ALTER TABLE smtp_queue RENAME TO old_queue")?;
        conn.execute_batch(&legacy)?;
        conn.execute_batch("INSERT INTO smtp_queue SELECT * FROM old_queue; DROP TABLE old_queue;
            ALTER TABLE smtp_delivery RENAME TO old_delivery;
            CREATE TABLE smtp_delivery(queue_id INTEGER,submission_id TEXT,recipient TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'queued',last_reply TEXT,updated_at TEXT NOT NULL DEFAULT(datetime('now')),
                UNIQUE(queue_id,recipient),UNIQUE(submission_id,recipient));
            INSERT INTO smtp_delivery SELECT queue_id,submission_id,recipient,status,last_reply,updated_at FROM old_delivery;
            DROP TABLE old_delivery;")?;
    }
    built.app_state.db.migrate().await?;
    built.app_state.db.migrate().await?;
    let entries = maild::smtp::queue::list(conn, 10).await?;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, queue_id);
    assert_eq!(entries[0].attempts, 1);
    assert_eq!(entries[0].blob_hash, hash);
    assert_eq!(
        entries[0].last_error.as_deref(),
        Some("451 stored diagnostic")
    );
    assert_eq!(status(&built, id).await?, original);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_smtputf8_peer_bounces_through_real_queue() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let mx = listener.local_addr()?;
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut io = BufReader::new(stream);
        io.get_mut()
            .write_all(b"220 mx.example.net\r\n")
            .await
            .unwrap();
        let mut line = String::new();
        io.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("EHLO"));
        io.get_mut()
            .write_all(b"250-mx.example.net\r\n250 8BITMIME\r\n")
            .await
            .unwrap();
        line.clear();
        assert_eq!(
            io.read_line(&mut line).await.unwrap(),
            0,
            "unsupported mail must not be offered"
        );
    });
    let tmp = tempfile::tempdir()?;
    let (built, _) = fixture(&tmp, Some(mx)).await?;
    let sender = account(&built, "élise@example.com").await?;
    let request = bus_cmd(
        "maild.submit",
        json!({"operation_id":"no-utf8-1","account":"élise@example.com",
        "to":["remote@remote.example.net"],"subject":"international sender","text":"body"}),
    );
    let db = &built.app_state.db;
    let (rc, body) = maild::bus::submission::dispatch(
        &request,
        db,
        &built.app_state.mailstore,
        &built.app_state.aliases_runtime,
        1024 * 1024,
    )
    .await;
    ensure!(rc == 0, "submit: {body}");
    let receipt: Value = serde_json::from_str(&body)?;
    let query = bus_cmd(
        "maild.submission.status",
        json!({"id":receipt["submission_id"]}),
    );
    timeout(Duration::from_secs(45), peer).await??;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let status: Value =
            serde_json::from_str(&maild::bus::submission::status(&query, db).await.1)?;
        if status["recipients"][0]["status"] == "bounced" && inbox_count(&built, sender)? == 1 {
            assert!(
                status["recipients"][0]["last_smtp_reply"]
                    .as_str()
                    .unwrap()
                    .contains("5.6.7")
            );
            assert_eq!(status["delivery_confirmed"], false);
            break;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "missing bounce/DSN: {status}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(maild::smtp::queue::list(&db.conn, 10).await?.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submission_status_tracks_real_smtp_replies_and_terminal_receipts() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let mx = listener.local_addr()?;
    let (release, ready) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        ready.await.unwrap();
        let mut mails = Vec::new();
        for _ in 0..3 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut io = BufReader::new(stream);
            io.get_mut()
                .write_all(b"220 mx.example.net\r\n")
                .await
                .unwrap();
            let mut data = false;
            loop {
                let mut line = String::new();
                if io.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let response = if data {
                    if line == ".\r\n" {
                        data = false;
                        "250 2.0.0 receiver stored message\r\n"
                    } else {
                        continue;
                    }
                } else if line.starts_with("EHLO") {
                    "250-mx.example.net\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n"
                } else if line.starts_with("MAIL") {
                    mails.push(line.clone());
                    "250 sender ok\r\n"
                } else if line.contains("<bounce@") {
                    "550 5.1.1 recipient gone\r\n"
                } else if line.contains("<later@") {
                    "451 4.7.1 try later\r\n"
                } else if line.starts_with("DATA") {
                    data = true;
                    "354 send data\r\n"
                } else if line.starts_with("QUIT") {
                    break;
                } else {
                    "250 ok\r\n"
                };
                io.get_mut().write_all(response.as_bytes()).await.unwrap();
            }
        }
        mails
    });
    let tmp = tempfile::tempdir()?;
    let (built, _) = fixture(&tmp, Some(mx)).await?;
    account(&built, "élise@example.com").await?;
    let db = &built.app_state.db;
    let request = bus_cmd(
        "maild.submit",
        json!({"operation_id":"wire-status-1","account":"élise@example.com",
        "to":["ok@remote.example.net","bounce@remote.example.net","later@remote.example.net"],"subject":"status","text":"body"}),
    );
    let (rc, body) = maild::bus::submission::dispatch(
        &request,
        db,
        &built.app_state.mailstore,
        &built.app_state.aliases_runtime,
        1024 * 1024,
    )
    .await;
    ensure!(rc == 0, "submit: {body}");
    let receipt: Value = serde_json::from_str(&body)?;
    let query = bus_cmd(
        "maild.submission.status",
        json!({"id":receipt["submission_id"]}),
    );
    let initial: Value = serde_json::from_str(&maild::bus::submission::status(&query, db).await.1)?;
    assert!(
        initial["recipients"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "queued")
    );
    release.send(()).unwrap();
    let mails = timeout(Duration::from_secs(45), peer).await??;
    assert_eq!(mails.len(), 3);
    assert!(mails.iter().all(|mail| mail.contains("SMTPUTF8")));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let final_status = loop {
        let (rc, body) = maild::bus::submission::status(&query, db).await;
        ensure!(rc == 0, "status: {body}");
        let value: Value = serde_json::from_str(&body)?;
        if value["recipients"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] != "queued")
        {
            break value;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "status not persisted"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let results = final_status["recipients"].as_array().unwrap();
    assert_eq!(
        results
            .iter()
            .map(|r| r["status"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["bounced", "deferred", "delivered"]
    );
    assert!(
        results[0]["last_smtp_reply"]
            .as_str()
            .unwrap()
            .contains("550 5.1.1")
    );
    assert!(
        results[1]["last_smtp_reply"]
            .as_str()
            .unwrap()
            .contains("451 4.7.1")
    );
    assert!(
        results[2]["last_smtp_reply"]
            .as_str()
            .unwrap()
            .contains("250 2.0.0 receiver stored")
    );
    assert_eq!(
        maild::smtp::queue::pending(&db.conn, 1).await?,
        vec!["later@remote.example.net"]
    );
    let reopened = maild::db::Db::connect(
        tmp.path().join("mail.db").to_str().unwrap(),
        tmp.path().join("blobs").to_str().unwrap(),
    )
    .await?;
    reopened.migrate().await?;
    assert_eq!(
        serde_json::from_str::<Value>(&maild::bus::submission::status(&query, &reopened).await.1)?,
        final_status
    );
    let wrong = bus_cmd(
        "maild.submission.status",
        json!({"id":uuid::Uuid::new_v4()}),
    );
    assert_eq!(maild::bus::submission::status(&wrong, db).await.0, 10);
    Ok(())
}
