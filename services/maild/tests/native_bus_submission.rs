// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Isolated actual noded → maild ABP submission. Never contacts an external MX.
//! Run ignored with MIXOS_NATIVE_MAIL_NODED and MIXOS_NATIVE_MAIL_FIXTURE_DIR.

use std::fs::File;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use ::bus::native_client::NodedClient;
use anyhow::{Context, Result, ensure};
use maild::config::Config;
use maild::mailstore::{ListOpts, MailStore, MailboxRole};
use maild::runtime::{RuntimeOpts, build_runtime};
use serde_json::json;

struct Broker(Child);
impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires an exact built native broker and persistent fixture directory"]
fn native_bus_submission_replays_across_actors_without_duplicate_mail() -> Result<()> {
    if std::env::var("MIXOS_NATIVE_MAIL_CHILD").as_deref() == Ok("1") {
        return tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?
            .block_on(run_child());
    }
    let binary = std::env::var("MIXOS_NATIVE_MAIL_NODED")?;
    let artifact_dir = std::env::var("MIXOS_NATIVE_MAIL_FIXTURE_DIR")?;
    std::fs::create_dir_all(&artifact_dir)?;
    // Retain failures as well as successes; worker collection owns cleanup.
    let root = tempfile::tempdir_in(&artifact_dir)?.keep();
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let node_config = root.join("node.conf.mix");
    std::fs::write(
        &node_config,
        json!({"node":"mail-fixture","wg_ip":"127.0.0.1",
        "noded":{"port":port,"unix_socket":root.join("broker.sock")}})
        .to_string(),
    )?;
    let broker_log = File::create(root.join("noded.log"))?;
    let _broker = Broker(
        Command::new(binary)
            .args(["serve", "--no-monitor", "--no-log"])
            .env("MIXOS_NODE_CONFIG", &node_config)
            .stdout(Stdio::from(broker_log.try_clone()?))
            .stderr(Stdio::from(broker_log))
            .spawn()?,
    );
    let output = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "native_bus_submission_replays_across_actors_without_duplicate_mail",
            "--ignored",
            "--nocapture",
        ])
        .env("MIXOS_NODE_CONFIG", &node_config)
        .env("MIXOS_NATIVE_MAIL_CHILD", "1")
        .env("MIXOS_NATIVE_MAIL_ROOT", &root)
        .env("MIXOS_NATIVE_MAIL_URL", format!("ws://127.0.0.1:{port}/ws"))
        .output()?;
    std::fs::write(root.join("maild-test.stdout"), &output.stdout)?;
    std::fs::write(root.join("maild-test.stderr"), &output.stderr)?;
    println!("NATIVE_MAIL_ARTIFACTS {}", root.display());
    ensure!(
        output.status.success(),
        "native child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

async fn connect(name: &str, url: &str) -> Result<NodedClient> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout(Duration::from_millis(500), NodedClient::connect(name, url))
            .await
        {
            Ok(Ok(client)) => return Ok(client),
            _ if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            _ => anyhow::bail!("native broker did not become ready"),
        }
    }
}

async fn call(
    client: &NodedClient,
    command: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(10), client.call("maild", command, args))
        .await
        .context("native command deadline")?
}

async fn cli(args: &[&str]) -> Result<String> {
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let socket = Path::new(&std::env::var("MIXOS_NATIVE_MAIL_ROOT")?).join("broker.sock");
    tokio::task::spawn_blocking(move || {
        let output = Command::new(env!("CARGO_BIN_EXE_maild"))
            .args(args)
            .env("MIXOS_BUS_SOCKET", socket)
            .env_remove("MIXOS_SESSION_FD")
            .output()?;
        ensure!(
            output.status.success(),
            "maild CLI failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8(output.stdout)?)
    })
    .await?
}

async fn run_child() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root = std::env::var("MIXOS_NATIVE_MAIL_ROOT")?;
    let root = Path::new(&root);
    let url = std::env::var("MIXOS_NATIVE_MAIL_URL")?;
    let config = Config {
        listen: "127.0.0.1:0".into(),
        base_url: "http://127.0.0.1".into(),
        hostname: "native-mail.example.test".into(),
        database_path: root.join("mail.sqlite").to_string_lossy().into_owned(),
        blob_dir: root.join("blobs").to_string_lossy().into_owned(),
        mds_dir: root.join("mds").to_string_lossy().into_owned(),
        spam_db_dir: Some(root.join("spam").to_string_lossy().into_owned()),
        rule_stats_dir: Some(root.join("rules").to_string_lossy().into_owned()),
        smtp_inbound: None,
        smtp_smtps: None,
        imap_imaps: None,
        spam_enabled: Some(false),
        ..Config::default()
    };
    let built = build_runtime(
        &config,
        RuntimeOpts {
            enable_bus: true,
            disable_outbound_delivery: true,
            ..RuntimeOpts::default()
        },
    )
    .await?;
    {
        let conn = built
            .app_state
            .db
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("database lock"))?;
        conn.execute("INSERT INTO accounts(email,password,name) VALUES('sender@example.test','test-hash','Sender'),('local@example.test','test-hash','Local')",[])?;
    }
    let first_actor = connect("native-mail-first", &url).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let help = tokio::time::timeout(
            Duration::from_millis(500),
            first_actor.call("maild", "HELP", json!(null)),
        )
        .await;
        if matches!(help,Ok(Ok(ref value)) if value.to_string().contains("maild.submit")) {
            break;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "maild submission manifest did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for email in ["sender@example.test", "local@example.test"] {
        call(
            &first_actor,
            "maild.accounts.seed_mailboxes",
            json!({"email":email}),
        )
        .await?;
    }
    let payload = json!({"operation_id":"native-once-1","account":"sender@example.test",
        "to":["local@example.test","remote@outside.test"],"subject":"Native fixture","text":"Actual ABP text ✓"});
    let first = call(&first_actor, "maild.submit", payload.clone()).await?;
    std::fs::write(
        root.join("first-receipt.json"),
        serde_json::to_vec_pretty(&first)?,
    )?;
    ensure!(
        first["status"] == "accepted" && first["delivery_confirmed"] == false,
        "invalid acceptance receipt"
    );
    let second_actor = connect("native-mail-reconnected", &url).await?;
    let replay = call(&second_actor, "maild.submit", payload.clone()).await?;
    std::fs::write(
        root.join("changed-actor-receipt.json"),
        serde_json::to_vec_pretty(&replay)?,
    )?;
    ensure!(
        first == replay,
        "actor-changed retry did not replay exact receipt"
    );
    let mut conflict = payload;
    conflict["text"] = json!("different payload");
    let conflict = call(&second_actor, "maild.submit", conflict).await;
    std::fs::write(
        root.join("conflict-result.json"),
        serde_json::to_vec_pretty(
            &json!({"error":conflict.as_ref().err().map(ToString::to_string)}),
        )?,
    )?;
    ensure!(conflict.is_err(), "conflicting operation was admitted");
    let local = maild::db::account::get_by_email(&built.app_state.db.conn, "local@example.test")
        .await?
        .context("local account")?;
    let inbox = built
        .app_state
        .mailstore
        .mailbox_by_role(local.id, MailboxRole::Inbox)?
        .context("local Inbox")?;
    let local_count = built
        .app_state
        .mailstore
        .list_emails_in_mailbox(local.id, inbox, ListOpts::default())?
        .len();
    let queued = maild::smtp::queue::list(&built.app_state.db.conn, 10).await?;
    ensure!(
        local_count == 1 && queued.len() == 1,
        "native replay duplicated delivery or queue"
    );
    ensure!(
        queued[0].to_addrs == ["remote@outside.test"],
        "incorrect remote recipient"
    );
    let actor:String = built.app_state.db.conn.lock().map_err(|_| anyhow::anyhow!("database lock"))?
        .query_row("SELECT actor FROM bus_submissions WHERE account='sender@example.test' AND operation_id='native-once-1'",[],|row| row.get(0))?;
    ensure!(
        actor == "native-mail-first",
        "replay replaced original actor audit"
    );
    std::fs::write(
        root.join("native-receipts.json"),
        serde_json::to_vec_pretty(&json!({"first":first,
        "changed_actor_replay":replay,"conflict_rejected":true,"local_count":local_count,
        "queue_count":queued.len(),"original_actor":actor,"outbound_delivery_disabled":true}))?,
    )?;
    let status = call(
        &second_actor,
        "maild.submission.status",
        json!({"id":first["submission_id"]}),
    )
    .await?;
    ensure!(
        status["recipients"][0]["status"] == "delivered"
            && status["recipients"][1]["status"] == "queued",
        "native status did not read real local/queue results"
    );
    // The legacy sender row has no props metadata. Show must still expose its id.
    let shown: serde_json::Value =
        serde_json::from_str(&cli(&["account", "show", "sender@example.test"]).await?)?;
    ensure!(
        shown["account_id"].as_i64().is_some(),
        "account show omitted numeric id"
    );
    for email in ["alice@example.com", "bob@example.com"] {
        cli(&["account", "add", email, "test-password"]).await?;
    }
    cli(&["alias", "add", "team@example.com", "alice@example.com"]).await?;
    ensure!(
        cli(&["alias", "add", "team@example.com", "bob@example.com"])
            .await
            .is_err(),
        "add silently repointed a live alias"
    );
    cli(&["alias", "remove", "team@example.com"]).await?;
    cli(&["alias", "add", "team@example.com", "bob@example.com"]).await?;
    let resurrected = call(&second_actor,"maild.submit",json!({"operation_id":"native-alias-resurrect",
        "account":"sender@example.test","to":["team@example.com"],"subject":"resurrect","text":"resurrect"})).await?;
    cli(&[
        "alias",
        "set",
        "team@example.com",
        "alice@example.com",
        "bob@example.com",
    ])
    .await?;
    cli(&["alias", "add", "@example.com", "alice@example.com"]).await?;
    for (operation, to) in [
        ("native-alias-fanout", "team@example.com"),
        ("native-mailbox-wins", "bob@example.com"),
        ("native-catchall", "unknown@example.com"),
    ] {
        let receipt = call(
            &second_actor,
            "maild.submit",
            json!({"operation_id":operation,
            "account":"sender@example.test","to":[to],"subject":operation,"text":"local aliases"}),
        )
        .await?;
        let status = call(
            &second_actor,
            "maild.submission.status",
            json!({"id":receipt["submission_id"]}),
        )
        .await?;
        ensure!(
            status["delivery_confirmed"] == true,
            "local alias receipt was not confirmed: {status}"
        );
    }
    let mut counts = Vec::new();
    for email in ["alice@example.com", "bob@example.com"] {
        let account = maild::db::account::get_by_email(&built.app_state.db.conn, email)
            .await?
            .context("CLI account")?;
        let inbox = built
            .app_state
            .mailstore
            .mailbox_by_role(account.id, MailboxRole::Inbox)?
            .context("CLI account Inbox")?;
        counts.push(
            built
                .app_state
                .mailstore
                .list_emails_in_mailbox(account.id, inbox, ListOpts::default())?
                .len(),
        );
        let shown: serde_json::Value =
            serde_json::from_str(&cli(&["account", "show", email]).await?)?;
        ensure!(
            shown["account_id"] == account.id,
            "account show printed the wrong numeric id"
        );
    }
    ensure!(
        counts == [2, 3],
        "CLI alias delivery precedence/fan-out wrong: {counts:?}"
    );
    let refused = cli(&["alias", "add", "outside@example.com", "nobody@example.net"])
        .await
        .unwrap_err();
    ensure!(
        refused.to_string().contains("off-site forwarding"),
        "non-local target error was unclear: {refused}"
    );
    let refusal = cli(&["bayesian", "stats", "999999"]).await.unwrap_err().to_string();
    assert_eq!(refusal.trim(), "maild CLI failed: maild.bayesian.stats failed (rc=10): account not found");
    ensure!(
        !root.join("spam/999999").exists(),
        "Bayes stats created an unknown corpus directory"
    );
    std::fs::write(
        root.join("cli-alias-receipts.json"),
        serde_json::to_vec_pretty(&json!({
        "resurrected":resurrected,"alice_inbox":counts[0],"bob_inbox":counts[1],
        "unknown_bayes_refused":true,"non_local_alias_refused":true}))?,
    )?;
    Ok(())
}
