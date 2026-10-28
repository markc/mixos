// SPDX-License-Identifier: MIT OR Apache-2.0
use anyhow::Result;
use maild::{db::Db, sieve::store};
use serde_json::json;

async fn fixture() -> Result<(tempfile::TempDir, Db)> {
    let tmp = tempfile::tempdir()?;
    let db = Db::connect(
        tmp.path().join("mail.db").to_str().unwrap(),
        tmp.path().join("blobs").to_str().unwrap(),
    )
    .await?;
    db.migrate().await?;
    db.conn.lock().unwrap().execute("INSERT INTO accounts(email,password) VALUES('bob@example.com','test'),('alice@example.com','test')",[])?;
    Ok((tmp, db))
}
#[tokio::test]
async fn versions_atomic_activation_isolation_and_optimistic_edits() -> Result<()> {
    let (_tmp, db) = fixture().await?;
    assert_eq!(store::put(&db, 1, "main", "keep;", Some(0)).await?, 1);
    assert_eq!(store::put(&db, 1, "main", "discard;", Some(1)).await?, 2);
    assert!(store::put(&db, 1, "main", "keep;", Some(1)).await.is_err());
    assert!(
        store::put(&db, 1, "main", "invalid;", Some(2))
            .await
            .is_err()
    );
    assert_eq!(store::get(&db, 1, "main").await?.source, "discard;");
    assert!(store::get(&db, 2, "main").await.is_err());
    store::activate(&db, 1, "main").await?;
    store::put(&db, 1, "other", "keep;", None).await?;
    let (a, b) = tokio::join!(
        store::activate(&db, 1, "main"),
        store::activate(&db, 1, "other")
    );
    a?;
    b?;
    assert_eq!(
        store::list(&db, 1)
            .await?
            .iter()
            .filter(|s| s.active)
            .count(),
        1
    );
    assert!(store::activate(&db, 1, "missing").await.is_err());
    assert_eq!(
        store::list(&db, 1)
            .await?
            .iter()
            .filter(|s| s.active)
            .count(),
        1
    );
    store::activate(&db, 1, "").await?;
    assert!(store::active(&db, 1).await?.is_none());
    store::delete(&db, 1, "main").await?;
    assert_eq!(store::put(&db, 1, "main", "keep;", None).await?, 4);
    Ok(())
}
#[tokio::test]
async fn quota_and_history_retention() -> Result<()> {
    let (_tmp, db) = fixture().await?;
    for n in 0..32 {
        store::put(&db, 1, &format!("script-{n}"), "keep;", None).await?;
    }
    assert!(store::have_space(&db, 1, "new", 1).await.is_err());
    assert!(store::put(&db, 1, "new", "keep;", None).await.is_err());
    store::have_space(&db, 1, "script-0", 100).await?;
    for _ in 0..20 {
        store::put(&db, 1, "script-0", "keep;", None).await?;
    }
    let count: i64 = db.conn.lock().unwrap().query_row(
        "SELECT count(*) FROM sieve_versions WHERE account_id=1 AND name='script-0'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(count, 16);
    Ok(())
}
#[tokio::test]
async fn script_names_accept_128_unicode_characters_and_reject_separators() -> Result<()> {
    let (_tmp, db) = fixture().await?;
    let name = "é".repeat(128);
    store::put(&db, 1, &name, "keep;", None).await?;
    assert_eq!(store::get(&db, 1, &name).await?.name, name);
    for name in [
        "x\u{2028}y".to_string(),
        "e\u{301}".to_string(),
        "é".repeat(129),
    ] {
        assert!(store::put(&db, 1, &name, "keep;", None).await.is_err());
    }
    Ok(())
}
#[tokio::test]
async fn deleted_name_churn_has_bounded_history_and_monotonic_versions() -> Result<()> {
    let (_tmp, db) = fixture().await?;
    let mut previous = 0;
    for n in 0..520 {
        let name = format!("churn-{n}");
        let version = store::put(&db, 1, &name, "keep;", None).await?;
        assert!(version > previous);
        previous = version;
        store::delete(&db, 1, &name).await?;
    }
    let count: i64 = db.conn.lock().unwrap().query_row(
        "SELECT count(*) FROM sieve_versions WHERE account_id=1",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(count, 512);
    assert!(store::put(&db, 1, "churn-0", "keep;", None).await? > previous);
    Ok(())
}
#[tokio::test]
async fn vacation_stamp_quota_expiration_and_fixed_size_handles() -> Result<()> {
    let (tmp,db) = fixture().await?;
    let mds = mds::SqliteCasMds::open(tmp.path().join("mds"))?;
    let hash = mds::Mds::put_blob(&mds,b"reply")?;
    db.conn.lock().unwrap().execute_batch("WITH RECURSIVE n(x) AS (SELECT 0 UNION ALL SELECT x+1 FROM n WHERE x<10004) INSERT INTO sieve_replies(account_id,sender,handle,sent_at) SELECT 1,'sender-'||x,'old',unixepoch()-1 FROM n;")?;
    assert!(store::queue_reply(&db,1,"Sender@example.org",&"handle".repeat(10000),7,hash).await?);
    {
        let conn = db.conn.lock().unwrap();
        assert_eq!(conn.query_row("SELECT count(*) FROM sieve_replies",[],|r| r.get::<_,i64>(0))?,store::MAX_REPLY_STAMPS as i64);
        assert!(conn.query_row("SELECT max(length(handle)) FROM sieve_replies",[],|r| r.get::<_,i64>(0))? <= 64);
        conn.execute("UPDATE sieve_replies SET sent_at=unixepoch()-32*86400",[])?;
    }
    assert!(store::queue_reply(&db,1,"Sender@example.org","new",7,hash).await?);
    assert_eq!(db.conn.lock().unwrap().query_row("SELECT count(*) FROM sieve_replies",[],|r| r.get::<_,i64>(0))?,1);
    let queued = maild::smtp::queue::list(&db.conn,10).await?;
    assert_eq!(queued.len(),2); assert_eq!(queued[0].to_addrs,vec!["Sender@example.org"]);
    Ok(())
}
#[tokio::test]
async fn bus_commands_use_shared_compiler_and_store() -> Result<()> {
    let (_tmp, db) = fixture().await?;
    let mut cmd = ::bus::IncomingCommand { generation:0,from:"operator".into(),command:"maild.sieve.put".into(),id:None,args:serde_json::Value::Null,headers:Default::default(),body:json!({"account":"bob@example.com","name":"main","source":"discard;","expected_version":0}).to_string() };
    let result = maild::bus::sieve::dispatch(&cmd, &db).await;
    assert_eq!(result.0, 0, "{}", result.1);
    assert_eq!(maild::bus::sieve::dispatch(&cmd, &db).await.0, 10);
    cmd.command = "maild.sieve.activate".into();
    cmd.body = json!({"account":"bob@example.com","name":"main"}).to_string();
    assert_eq!(maild::bus::sieve::dispatch(&cmd, &db).await.0, 0);
    assert_eq!(store::active(&db, 1).await?.unwrap().source, "discard;");
    cmd.command = "maild.sieve.get".into();
    let result = maild::bus::sieve::dispatch(&cmd, &db).await;
    assert_eq!(result.0, 0);
    assert!(result.1.contains("discard;"));
    cmd.command = "maild.sieve.list".into();
    cmd.body = json!({"account":"alice@example.com"}).to_string();
    assert_eq!(
        maild::bus::sieve::dispatch(&cmd, &db).await,
        (0, "{\"scripts\":[]}".into())
    );
    Ok(())
}
