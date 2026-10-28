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
    command(&mut io, "MAIL FROM:<sender@example.org>", "250").await?;
    command(&mut io, &format!("RCPT TO:<{address}>"), "553").await?;
    command(&mut io, "RCPT TO:<alice@example.com>", "250").await?;
    command(&mut io, "DATA", "354").await?;
    command(
        &mut io,
        "From: sender@example.org\r\nSubject: café\r\n\r\nbody\r\n.",
        "550",
    )
    .await?;
    command(&mut io, "MAIL FROM:<sender@example.org> BODY=8BITMIME", "250").await?;
    command(&mut io, "RCPT TO:<alice@example.com>", "250").await?;
    command(&mut io, "DATA", "354").await?;
    command(&mut io, "MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\nContent-Disposition: inline; filename=\"café.txt\"\r\n\r\nbody\r\n--x--\r\n.", "550").await?;
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
        &format!("MAIL FROM:<{address}> SMTPUTF8 BODY=8BITMIME"),
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
    assert!(identities.iter().any(|identity| identity["email"] == "team@example.com"));
    assert!(!identities.iter().any(|identity| identity["email"] == "@example.com"));
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
    assert_eq!(status["recipients"].as_array().unwrap().len(), 2);
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
