// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real STARTTLS/implicit-TLS ManageSieve and SMTP delivery using production
//! runtime wiring, plus duplex malformed-frame/authentication tests.
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use maild::{
    config::{Config, ListenSpec},
    db,
    mailstore::{ListOpts, MailStore},
    runtime::{RuntimeOpts, build_runtime},
    sieve::{server, store},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpStream,
};

async fn response<S: AsyncRead + Unpin>(io: &mut BufReader<S>) -> Result<String> {
    let mut output = String::new();
    loop {
        let mut line = String::new();
        ensure!(
            tokio::time::timeout(Duration::from_secs(15), io.read_line(&mut line)).await?? > 0,
            "unexpected EOF: {output}"
        );
        let done = ["OK", "NO", "BYE"].iter().any(|s| line.starts_with(s));
        output.push_str(&line);
        if done {
            return Ok(output);
        }
    }
}
async fn send<S: AsyncRead + AsyncWrite + Unpin>(
    io: &mut BufReader<S>,
    command: &str,
) -> Result<String> {
    io.get_mut().write_all(command.as_bytes()).await?;
    response(io).await
}
async fn smtp_line<S: AsyncRead + Unpin>(io: &mut BufReader<S>) -> Result<String> {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(30), io.read_line(&mut line)).await??;
    Ok(line)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn starttls_upload_activate_then_real_smtp_delivery_and_rejection() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tmp = tempfile::tempdir()?;
    let certified = rcgen::generate_simple_self_signed(vec!["mail.example.com".into()])?;
    let cert_path = tmp.path().join("cert.pem");
    let key_path = tmp.path().join("key.pem");
    std::fs::write(&cert_path, certified.cert.pem())?;
    std::fs::write(&key_path, certified.key_pair.serialize_pem())?;
    let filter_path = tmp.path().join("inbound.mix");
    std::fs::write(
        &filter_path,
        "-- SPDX-License-Identifier: MIT OR Apache-2.0\nif $TO == \"alice@example.com\" then\n  if $SPAM_VERDICT == \"SPAM\" then return \"RecipientPolicy\" end\n  return \"WrongVerdict\"\nend\nreturn \"Global\"\n",
    )?;
    let mut cfg = Config {
        hostname: "mail.example.com".into(),
        database_path: tmp.path().join("mail.db").display().to_string(),
        blob_dir: tmp.path().join("blobs").display().to_string(),
        mds_dir: tmp.path().join("mds").display().to_string(),
        spam_db_dir: Some(tmp.path().join("spam").display().to_string()),
        rule_stats_dir: Some(tmp.path().join("stats").display().to_string()),
        smtp_inbound: Some(ListenSpec::Single("127.0.0.1:0".into())),
        smtp_smtps: None,
        sieve_listen: Some(ListenSpec::Single("127.0.0.1:0".into())),
        tls_cert: Some(cert_path.display().to_string()),
        tls_key: Some(key_path.display().to_string()),
        inbound_filter: Some(filter_path.display().to_string()),
        ..Config::default()
    };
    let built = build_runtime(
        &cfg,
        RuntimeOpts {
            enable_bus: false,
            disable_outbound_delivery: true,
            ..Default::default()
        },
    )
    .await?;
    let db = &built.app_state.db;
    let ms = &built.app_state.mailstore;
    let hash = bcrypt::hash("test-password", 4)?;
    db.conn.lock().unwrap().execute(
        "INSERT INTO accounts(email,password,spam_enabled) VALUES('bob@example.com',?1,0)",
        [hash],
    )?;
    let account = db::account::get_by_email(&db.conn, "bob@example.com")
        .await?
        .unwrap();
    db.conn.lock().unwrap().execute("INSERT INTO accounts(email,password,spam_enabled,spam_threshold) VALUES('alice@example.com','unused',1,0)",[])?;
    let alice = db::account::get_by_email(&db.conn,"alice@example.com").await?.unwrap();
    let alice_inbox = ms.create_mailbox(alice.id,"Inbox",None,mds::ContainerAttrs { special_use:Some("\\Inbox".into()),subscribed:true,extra:serde_json::json!({}) })?;
    let alice_target = ms.create_mailbox(alice.id,"RecipientPolicy",None,mds::ContainerAttrs { special_use:None,subscribed:false,extra:serde_json::json!({}) })?;
    store::put(db,alice.id,"spam-check",r#"if header :is "X-Mixos-Spam-Verdict" "SPAM" { keep; } else { discard; }"#,None).await?;
    store::activate(db,alice.id,"spam-check").await?;
    ms.create_mailbox(
        account.id,
        "Inbox",
        None,
        mds::ContainerAttrs {
            special_use: Some("\\Inbox".into()),
            subscribed: true,
            extra: serde_json::json!({}),
        },
    )?;
    let global = ms.create_mailbox(
        account.id,
        "Global",
        None,
        mds::ContainerAttrs {
            special_use: None,
            subscribed: false,
            extra: serde_json::json!({}),
        },
    )?;

    let mut roots = rustls::RootCertStore::empty();
    roots.add(certified.cert.der().clone())?;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ));
    let mut clear = BufReader::new(TcpStream::connect(built.sieve_handle.addrs[0]).await?);
    assert!(response(&mut clear).await?.contains("STARTTLS"));
    assert!(send(&mut clear, "CAPABILITY\r\n").await?.contains("SIEVE"));
    let auth = format!(
        "AUTHENTICATE \"PLAIN\" \"{}\"\r\n",
        STANDARD.encode(b"\0bob@example.com\0test-password")
    );
    assert!(
        send(&mut clear, &auth)
            .await?
            .starts_with("NO (ENCRYPT-NEEDED)")
    );
    assert!(send(&mut clear, "STARTTLS\r\n").await?.starts_with("OK"));
    let tls = connector
        .connect(
            rustls::pki_types::ServerName::try_from("mail.example.com")?,
            clear.into_inner(),
        )
        .await?;
    let mut io = BufReader::new(tls);
    let greeting = response(&mut io).await?;
    assert!(greeting.contains("PLAIN LOGIN"));
    assert!(!greeting.contains("STARTTLS"));
    assert!(send(&mut io, &auth).await?.starts_with("OK"));
    assert!(
        send(&mut io, "HAVESPACE \"main\" 65537\r\n")
            .await?
            .starts_with("NO (QUOTA/MAXSIZE)")
    );
    assert!(
        send(&mut io, "CHECKSCRIPT \"unknown;\"\r\n")
            .await?
            .starts_with("NO")
    );
    let script = "require [\"fileinto\",\"mailbox\",\"imap4flags\"];\r\nif header :is \"X-Mixos-Spam-Verdict\" \"UNKNOWN\" { fileinto :create :flags \"\\\\Seen\" \"Projects\"; } else { discard; }\r\n";
    assert!(
        send(
            &mut io,
            &format!("CHECKSCRIPT {{{}+}}\r\n{script}\r\n", script.len())
        )
        .await?
        .starts_with("OK")
    );
    assert!(
        send(
            &mut io,
            &format!("PUTSCRIPT \"main\" {{{}+}}\r\n{script}\r\n", script.len())
        )
        .await?
        .starts_with("OK")
    );
    assert!(
        send(&mut io, "LISTSCRIPTS\r\n")
            .await?
            .contains("\"main\"\r\n")
    );
    assert!(
        send(&mut io, "SETACTIVE \"main\"\r\n")
            .await?
            .starts_with("OK")
    );
    assert!(
        send(&mut io, "LISTSCRIPTS\r\n")
            .await?
            .contains("\"main\" ACTIVE")
    );
    assert!(
        send(&mut io, "DELETESCRIPT \"main\"\r\n")
            .await?
            .starts_with("NO (ACTIVE)")
    );
    io.get_mut().write_all(b"GETSCRIPT \"main\"\r\n").await?;
    let marker = smtp_line(&mut io).await?;
    assert_eq!(marker, format!("{{{}}}\r\n", script.len()));
    let mut got = vec![0; script.len()];
    io.read_exact(&mut got).await?;
    assert_eq!(got, script.as_bytes());
    assert_eq!(smtp_line(&mut io).await?, "\r\n");
    assert!(response(&mut io).await?.starts_with("OK"));
    assert!(
        send(&mut io, "RENAMESCRIPT \"main\" \"renamed\"\r\n")
            .await?
            .starts_with("OK")
    );
    assert!(store::get(db, account.id, "renamed").await?.active);
    assert!(
        send(&mut io, "NOOP \"tag-1\"\r\n")
            .await?
            .starts_with("OK (TAG \"tag-1\")")
    );

    let mut smtp = BufReader::new(TcpStream::connect(built.smtp_handle.inbound_addrs[0]).await?);
    assert!(smtp_line(&mut smtp).await?.starts_with("220"));
    smtp.get_mut()
        .write_all(b"HELO sender.example.org\r\n")
        .await?;
    assert!(smtp_line(&mut smtp).await?.starts_with("250"));
    async fn message(io: &mut BufReader<TcpStream>) -> Result<String> {
        for (cmd, code) in [
            ("MAIL FROM:<sender@example.org>\r\n", "250"),
            ("RCPT TO:<bob@example.com>\r\n", "250"),
            ("DATA\r\n", "354"),
        ] {
            io.get_mut().write_all(cmd.as_bytes()).await?;
            let line = smtp_line(io).await?;
            ensure!(line.starts_with(code), "{line}");
        }
        io.get_mut().write_all(b"From: sender@example.org\r\nTo: bob@example.com\r\nSubject: wire Sieve\r\n\r\nhello\r\n.\r\n").await?;
        smtp_line(io).await
    }
    assert!(message(&mut smtp).await?.starts_with("250"));
    let projects = ms.mailbox_by_name(account.id, "Projects")?.unwrap();
    let messages = ms.list_emails_in_mailbox(account.id, projects, ListOpts::default())?;
    assert_eq!(messages.len(), 1);
    assert!(
        ms.list_emails_in_mailbox(account.id, global, ListOpts::default())?
            .is_empty()
    );
    assert_ne!(messages[0].keywords.0 & mds::Flags::SEEN, 0);
    let redirect = r#"redirect "alice@example.com";"#;
    assert!(send(&mut io,&format!("PUTSCRIPT \"renamed\" {{{}+}}\r\n{redirect}\r\n",redirect.len())).await?.starts_with("OK"));
    assert!(message(&mut smtp).await?.starts_with("250"));
    assert_eq!(ms.list_emails_in_mailbox(alice.id,alice_target,ListOpts::default())?.len(),1);
    assert!(ms.list_emails_in_mailbox(alice.id,alice_inbox,ListOpts::default())?.is_empty());
    assert!(
        send(
            &mut io,
            "PUTSCRIPT \"renamed\" \"require \\\"ereject\\\"; ereject \\\"Denied\\\";\"\r\n"
        )
        .await?
        .starts_with("OK")
    );
    assert!(message(&mut smtp).await?.starts_with("550 5.7.1 Denied"));
    assert!(send(&mut io, "SETACTIVE \"\"\r\n").await?.starts_with("OK"));
    assert!(message(&mut smtp).await?.starts_with("250"));
    assert_eq!(
        ms.list_emails_in_mailbox(account.id, global, ListOpts::default())?
            .len(),
        1
    );
    assert!(
        send(&mut io, "DELETESCRIPT \"renamed\"\r\n")
            .await?
            .starts_with("OK")
    );
    assert!(send(&mut io, "LOGOUT\r\n").await?.starts_with("OK"));

    // The same identity also serves the optional implicit TLS mode.
    cfg.sieve_implicit_tls = true;
    let resolver =
        maild::tls::SniCertResolver::from_config(&cfg.resolve_tls_identities(), cfg.strict_sni())?;
    let handle = server::start(
        db.clone(),
        &cfg,
        maild::tls::new_tls_slot(Some(Arc::new(resolver))),
        Arc::new(maild::tls::ServerConfigCache::new()),
    )
    .await?;
    let tcp = TcpStream::connect(handle.addrs[0]).await?;
    let tls = connector
        .connect(
            rustls::pki_types::ServerName::try_from("mail.example.com")?,
            tcp,
        )
        .await?;
    let mut io = BufReader::new(tls);
    assert!(response(&mut io).await?.contains("PLAIN LOGIN"));
    assert!(send(&mut io, &auth).await?.starts_with("OK"));
    assert!(send(&mut io, "LOGOUT\r\n").await?.starts_with("OK"));
    Ok(())
}

async fn test_db() -> Result<(tempfile::TempDir, db::Db)> {
    let tmp = tempfile::tempdir()?;
    let db = db::Db::connect(
        tmp.path().join("mail.db").to_str().unwrap(),
        tmp.path().join("blobs").to_str().unwrap(),
    )
    .await?;
    db.migrate().await?;
    let hash = bcrypt::hash("test-password", 4)?;
    db.conn.lock().unwrap().execute(
        "INSERT INTO accounts(email,password) VALUES('bob@example.com',?1)",
        [hash],
    )?;
    Ok((tmp, db))
}
#[tokio::test]
async fn login_challenge_authzid_refusal_and_account_isolation() -> Result<()> {
    let (_tmp, db) = test_db().await?;
    let (client, stream) = tokio::io::duplex(128 * 1024);
    let db2 = db.clone();
    let task = tokio::spawn(async move { server::session(stream, &db2, true).await });
    let mut io = BufReader::new(client);
    response(&mut io).await?;
    let bad = format!(
        "AUTHENTICATE \"PLAIN\" \"{}\"\r\n",
        STANDARD.encode(b"other@example.com\0bob@example.com\0test-password")
    );
    assert!(send(&mut io, &bad).await?.starts_with("NO"));
    io.get_mut()
        .write_all(b"AUTHENTICATE \"LOGIN\"\r\n")
        .await?;
    assert!(smtp_line(&mut io).await?.contains("VXNlcm5hbWU6"));
    io.get_mut()
        .write_all(format!("\"{}\"\r\n", STANDARD.encode("bob@example.com")).as_bytes())
        .await?;
    assert!(smtp_line(&mut io).await?.contains("UGFzc3dvcmQ6"));
    assert!(
        send(
            &mut io,
            &format!("\"{}\"\r\n", STANDARD.encode("test-password"))
        )
        .await?
        .starts_with("OK")
    );
    assert!(
        send(&mut io, "PUTSCRIPT \"../other/main\" \"keep;\"\r\n")
            .await?
            .starts_with("OK")
    );
    assert_eq!(store::list(&db, 1).await?.len(), 1);
    assert!(store::list(&db, 2).await?.is_empty());
    assert!(send(&mut io, "LOGOUT\r\n").await?.starts_with("OK"));
    task.await??;
    Ok(())
}
#[tokio::test]
async fn oversized_literal_disconnects_without_allocating_or_writing_script() -> Result<()> {
    let (_tmp, db) = test_db().await?;
    let (client, stream) = tokio::io::duplex(1024);
    let db2 = db.clone();
    let task = tokio::spawn(async move { server::session(stream, &db2, true).await });
    let mut io = BufReader::new(client);
    response(&mut io).await?;
    assert!(
        send(&mut io, "PUTSCRIPT \"main\" {999999999+}\r\n")
            .await?
            .starts_with("BYE")
    );
    assert!(store::list(&db, 1).await?.is_empty());
    task.await??;
    Ok(())
}
