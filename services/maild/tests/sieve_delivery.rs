// SPDX-License-Identifier: MIT OR Apache-2.0
use anyhow::Result;
use maild::{
    db::{self, Db},
    mailstore::{ListOpts, MailStore, MailboxRole, SqliteMailStore},
    sieve::{
        delivery::{self, Delivery},
        store,
    },
};
use mds::{Flags, Mds, Tags};
use std::sync::Arc;

struct Fixture {
    _tmp: tempfile::TempDir,
    db: Db,
    ms: Arc<SqliteMailStore>,
    account: db::account::Account,
}
impl Fixture {
    async fn new() -> Result<Self> {
        let tmp = tempfile::tempdir()?;
        let db = Db::connect(
            tmp.path().join("mail.db").to_str().unwrap(),
            tmp.path().join("blobs").to_str().unwrap(),
        )
        .await?;
        db.migrate().await?;
        db.conn.lock().unwrap().execute("INSERT INTO accounts(email,password) VALUES('bob@example.com','test'),('alice@example.com','test')",[])?;
        let account = db::account::get_by_email(&db.conn, "bob@example.com")
            .await?
            .unwrap();
        let ms = Arc::new(SqliteMailStore::new(Arc::new(mds::SqliteCasMds::open(
            tmp.path().join("mds"),
        )?)));
        for id in [1, 2] {
            for (name, role) in [
                ("Inbox", "\\Inbox"),
                ("Sent", "\\Sent"),
                ("Drafts", "\\Drafts"),
                ("Junk", "\\Junk"),
            ] {
                ms.create_mailbox(
                    id,
                    name,
                    None,
                    mds::ContainerAttrs {
                        special_use: Some(role.into()),
                        subscribed: true,
                        extra: serde_json::json!({}),
                    },
                )?;
            }
        }
        Ok(Self {
            _tmp: tmp,
            db,
            ms,
            account,
        })
    }
    async fn script(&self, source: &str) -> Result<()> {
        store::put(&self.db, self.account.id, "main", source, None).await?;
        store::activate(&self.db, self.account.id, "main").await
    }
    async fn deliver(
        &self,
        bytes: &[u8],
        external: bool,
        smtp: bool,
    ) -> Result<Option<mds::ItemId>> {
        let domains = ["example.com".to_string()];
        delivery::deliver(
            &self.db,
            &self.ms,
            Delivery {
                account: &self.account,
                from: "sender@example.org",
                to: "bob@example.com",
                data: bytes,
                default_mailbox: self
                    .ms
                    .mailbox_by_role(self.account.id, MailboxRole::Inbox)?
                    .unwrap(),
                default_tags: Tags::new(),
                external_domains: if external { &domains } else { &[] },
                sender_verified: true,
                spam: false,
                smtp_reject: smtp,
                routing: None,
            },
        )
        .await
    }
    fn count(&self, name: &str) -> usize {
        self.ms
            .mailbox_by_name(self.account.id, name)
            .unwrap()
            .map(|id| {
                self.ms
                    .list_emails_in_mailbox(self.account.id, id, ListOpts::default())
                    .unwrap()
                    .len()
            })
            .unwrap_or(0)
    }
}
const MESSAGE: &[u8] = b"From: sender@example.org\r\nTo: bob@example.com\r\nSubject: test\r\nMessage-ID: <one@example.org>\r\n\r\nHello\r\n";

#[tokio::test]
async fn fileinto_create_copy_and_flags_use_real_mailstore() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["fileinto","mailbox","copy","imap4flags"]; fileinto :create :copy :flags ["\\Seen","project"] "Projects";"#).await?;
    f.deliver(MESSAGE, false, false).await?;
    assert_eq!(f.count("Projects"), 1);
    assert_eq!(f.count("Inbox"), 1);
    let target = f.ms.mailbox_by_name(f.account.id, "Projects")?.unwrap();
    let item =
        f.ms.list_emails_in_mailbox(f.account.id, target, ListOpts::default())?[0].clone();
    // Read via the storage adapter so both IMAP/JMAP see the same keywords.
    let record = f.ms.get_email(f.account.id, item.id)?;
    assert_ne!(record.keywords.0 & Flags::SEEN, 0);
    assert!(record.tags.iter().any(|t| t == "project"));
    Ok(())
}
#[tokio::test]
async fn repeated_filings_merge_flags_and_keywords() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["imap4flags","fileinto"]; keep :flags ["\\Seen","first"]; fileinto :flags ["\\Flagged","second"] "INBOX";"#).await?;
    f.deliver(MESSAGE,false,false).await?;
    assert_eq!(f.count("Inbox"),1);
    let inbox = f.ms.mailbox_by_role(f.account.id,MailboxRole::Inbox)?.unwrap();
    let items = f.ms.list_emails_in_mailbox(f.account.id,inbox,ListOpts::default())?;
    let record = f.ms.get_email(f.account.id,items[0].id)?;
    assert_eq!(record.keywords.0 & (Flags::SEEN | Flags::FLAGGED), Flags::SEEN | Flags::FLAGGED);
    assert!(record.tags.contains("first"));
    assert!(record.tags.contains("second"));
    Ok(())
}
#[tokio::test]
async fn mailboxexists_resolves_nested_account_owned_writable_mailboxes() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["fileinto","mailbox"]; fileinto :create "INBOX/Projects";"#).await?;
    f.deliver(MESSAGE,false,false).await?;
    store::put(&f.db,2,"other",r#"require ["fileinto","mailbox"]; fileinto :create "OtherAccount";"#,None).await?;
    store::activate(&f.db,2,"other").await?;
    f.script(r#"redirect "alice@example.com";"#).await?;
    f.deliver(MESSAGE,false,false).await?;
    f.ms.create_mailbox(f.account.id,"ReadOnly",None,mds::ContainerAttrs {special_use:None,subscribed:false,extra:serde_json::json!({"jmap_my_rights":{"may_add_items":false}})})?;
    f.script(r#"require "mailbox"; if allof(mailboxexists ["inbox","INBOX/Projects"],not mailboxexists "OtherAccount",not mailboxexists "ReadOnly") { discard; }"#).await?;
    assert!(f.deliver(MESSAGE,false,false).await?.is_none());
    assert_eq!(f.count("Inbox"),0);
    Ok(())
}
#[tokio::test]
async fn refused_redirect_cancels_other_actions_and_keeps() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"redirect "outsider@example.net"; discard;"#)
        .await?;
    assert!(f.deliver(MESSAGE, false, false).await?.is_some());
    assert_eq!(f.count("Inbox"), 1);
    assert!(maild::smtp::queue::list(&f.db.conn, 10).await?.is_empty());
    Ok(())
}
#[tokio::test]
async fn fileinto_nested_names_resolve_parent_and_inbox_role() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["fileinto","mailbox"]; fileinto :create "INBOX/Projects/Work";"#)
        .await?;
    f.deliver(MESSAGE, false, false).await?;
    let inbox =
        f.ms.mailbox_by_role(f.account.id, MailboxRole::Inbox)?
            .unwrap();
    let boxes = f.ms.list_mailboxes(f.account.id)?;
    let projects = boxes
        .iter()
        .find(|m| m.parent == Some(inbox) && m.name == "Projects")
        .unwrap();
    let work = boxes
        .iter()
        .find(|m| m.parent == Some(projects.id) && m.name == "Work")
        .unwrap();
    assert_eq!(
        f.ms.list_emails_in_mailbox(f.account.id, work.id, ListOpts::default())?
            .len(),
        1
    );
    Ok(())
}
#[tokio::test]
async fn local_redirect_runs_destination_script_and_bounds_cycles() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"redirect "alice@example.com";"#).await?;
    store::put(
        &f.db,
        2,
        "main",
        r#"require ["fileinto","mailbox"]; fileinto :create "Redirected";"#,
        None,
    )
    .await?;
    store::activate(&f.db, 2, "main").await?;
    assert!(f.deliver(MESSAGE, false, false).await?.is_none());
    let mailbox = f.ms.mailbox_by_name(2, "Redirected")?.unwrap();
    assert_eq!(
        f.ms.list_emails_in_mailbox(2, mailbox, ListOpts::default())?
            .len(),
        1
    );
    store::put(&f.db, 2, "main", r#"redirect "bob@example.com";"#, None).await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        f.deliver(MESSAGE, false, false),
    )
    .await??;
    assert!(maild::smtp::queue::list(&f.db.conn, 10).await?.is_empty());
    Ok(())
}
#[tokio::test]
async fn allowed_external_redirect_uses_native_outbound_queue() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"redirect "outsider@example.net";"#).await?;
    assert!(f.deliver(MESSAGE, true, false).await?.is_none());
    let queued = maild::smtp::queue::list(&f.db.conn, 10).await?;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].to_addrs, vec!["outsider@example.net"]);
    assert_eq!(queued[0].from_addr, "sender@example.org");
    Ok(())
}
#[tokio::test]
async fn corrupt_script_and_missing_mailbox_fall_back_to_keep() -> Result<()> {
    let f = Fixture::new().await?;
    f.script("keep;").await?;
    f.db.conn
        .lock()
        .unwrap()
        .execute("UPDATE sieve_scripts SET source='broken {'", [])?;
    f.deliver(MESSAGE, false, false).await?;
    assert_eq!(f.count("Inbox"), 1);
    f.script(r#"require "fileinto"; fileinto "missing";"#)
        .await?;
    f.deliver(MESSAGE, false, false).await?;
    assert_eq!(f.count("Inbox"), 2);
    Ok(())
}
#[tokio::test]
async fn runtime_error_rolls_back_planned_redirect() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"redirect "alice@example.com"; redirect "b@example.com"; redirect "c@example.com"; redirect "d@example.com"; redirect "e@example.com";"#).await?;
    f.deliver(MESSAGE, false, false).await?;
    assert_eq!(f.count("Inbox"), 1);
    let inbox = f.ms.mailbox_by_role(2, MailboxRole::Inbox)?.unwrap();
    assert!(
        f.ms.list_emails_in_mailbox(2, inbox, ListOpts::default())?
            .is_empty()
    );
    Ok(())
}
#[tokio::test]
async fn vacation_once_per_sender_survives_concurrency_and_restart() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require "vacation"; vacation :days 2 :subject "Away" :from "bob@example.com" "Back tomorrow";"#).await?;
    let (a, b) = tokio::join!(
        f.deliver(MESSAGE, true, false),
        f.deliver(MESSAGE, true, false)
    );
    a?;
    b?;
    let queued = maild::smtp::queue::list(&f.db.conn, 10).await?;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].from_addr, "");
    let bytes = f.ms.mds().get_blob(&queued[0].blob_hash.unwrap())?;
    let message = mail_parser::MessageParser::default().parse(&bytes).unwrap();
    assert_eq!(
        message.header("Auto-Submitted").and_then(|h| h.as_text()),
        Some("auto-replied")
    );
    assert_eq!(message.subject(), Some("Away"));
    let reopened = Db::connect(
        f._tmp.path().join("mail.db").to_str().unwrap(),
        f._tmp.path().join("blobs").to_str().unwrap(),
    )
    .await?;
    assert!(
        !store::reply_due(
            &reopened,
            f.account.id,
            "SENDER@example.org",
            &maild::sieve::compile(&store::get(&f.db, 1, "main").await?.source)?
                .evaluate(maild::sieve::Context {
                    data: MESSAGE,
                    from: "sender@example.org",
                    to: "bob@example.com"
                })?
                .vacation
                .unwrap()
                .handle,
            2
        )
        .await?
    );
    f.db.conn
        .lock()
        .unwrap()
        .execute("UPDATE sieve_replies SET sent_at=unixepoch()-3*86400", [])?;
    f.deliver(MESSAGE, true, false).await?;
    assert_eq!(maild::smtp::queue::list(&f.db.conn, 10).await?.len(), 2);
    Ok(())
}
#[tokio::test]
async fn vacation_suppresses_lists_autoresponses_bcc_and_offsite_policy() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require "vacation"; vacation "Away";"#).await?;
    f.deliver(MESSAGE, false, false).await?;
    for header in [
        "Auto-Submitted: auto-replied\r\n",
        "Auto-Submitted: no\r\nAuto-Submitted: auto-replied\r\n",
        "Precedence: normal\r\nPrecedence: bulk\r\n",
        "List-Id: list.example.org\r\n",
        "Precedence: bulk\r\n",
    ] {
        let mut bytes = header.as_bytes().to_vec();
        bytes.extend_from_slice(MESSAGE);
        f.deliver(&bytes, true, false).await?;
    }
    f.deliver(
        b"From: sender@example.org\r\nTo: other@example.com\r\n\r\nBcc\r\n",
        true,
        false,
    )
    .await?;
    assert!(maild::smtp::queue::list(&f.db.conn, 10).await?.is_empty());
    Ok(())
}
#[tokio::test]
async fn reject_uses_smtp_when_possible_else_null_sender_dsn() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require "ereject"; ereject "Denied";"#).await?;
    let error = f.deliver(MESSAGE, false, true).await.unwrap_err();
    assert!(error.downcast_ref::<delivery::Rejected>().is_some());
    f.deliver(MESSAGE, false, false).await?;
    assert_eq!(f.count("Inbox"), 0);
    let queued = maild::smtp::queue::list(&f.db.conn, 10).await?;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].from_addr, "");
    let bytes = f.ms.mds().get_blob(&queued[0].blob_hash.unwrap())?;
    assert!(String::from_utf8_lossy(&bytes).contains("Action: failed"));
    assert!(String::from_utf8_lossy(&bytes).contains("report-type=delivery-status"));
    Ok(())
}
#[tokio::test]
async fn native_submission_runs_recipient_sieve() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["fileinto","mailbox"]; fileinto :create "FromSubmission";"#)
        .await?;
    let aliases = Arc::new(::props::Runtime::new(
        "maild",
        maild::props::aliases::spec(::props::Hooks::noop()),
        Arc::new(::props::MemoryStore::new("maild")),
    ));
    let cmd = ::bus::IncomingCommand { generation:0,from:"test".into(),command:"maild.submit".into(),id:None,args:serde_json::Value::Null,headers:Default::default(),body:serde_json::json!({"operation_id":"sieve-submission","account":"alice@example.com","to":["bob@example.com"],"subject":"local sieve","text":"hello"}).to_string() };
    let result = maild::bus::submission::dispatch(&cmd, &f.db, &f.ms, &aliases, 1024 * 1024).await;
    assert_eq!(result.0, 0, "{}", result.1);
    assert_eq!(f.count("FromSubmission"), 1);
    assert_eq!(f.count("Inbox"), 0);
    Ok(())
}
#[tokio::test]
async fn unicode_ereject_uses_ascii_smtp_and_reject_preserves_reason_in_dsn() -> Result<()> {
    let f = Fixture::new().await?;
    f.script("require \"ereject\"; ereject \"Denied café\";")
        .await?;
    let error = f.deliver(MESSAGE, false, true).await.unwrap_err();
    assert!(
        error
            .downcast_ref::<delivery::Rejected>()
            .unwrap()
            .0
            .is_ascii()
    );
    f.script("require \"reject\"; reject \"Denied café\";")
        .await?;
    assert!(f.deliver(MESSAGE, false, true).await?.is_none());
    let queued = maild::smtp::queue::list(&f.db.conn, 10).await?;
    assert_eq!(queued.len(), 1);
    let bytes = f.ms.mds().get_blob(&queued[0].blob_hash.unwrap())?;
    assert!(String::from_utf8_lossy(&bytes).contains("Denied café"));
    Ok(())
}
#[tokio::test]
async fn vacation_mime_entity_preserves_body_and_refuses_envelope_header_injection() -> Result<()> {
    let f = Fixture::new().await?;
    f.script("require \"vacation\"; vacation :mime text:\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Away</p>\r\n.\r\n;").await?;
    f.deliver(MESSAGE, true, false).await?;
    let queued = maild::smtp::queue::list(&f.db.conn, 10).await?;
    assert_eq!(queued.len(), 1);
    let bytes = f.ms.mds().get_blob(&queued[0].blob_hash.unwrap())?;
    let message = mail_parser::MessageParser::default().parse(&bytes).unwrap();
    assert!(message.body_html(0).unwrap().contains("<p>Away</p>"));
    assert_eq!(
        message.from().unwrap().first().unwrap().address(),
        Some("bob@example.com")
    );
    f.script("require \"vacation\"; vacation :mime text:\r\nFrom: forged@example.org\r\nContent-Type: text/plain\r\n\r\nInjected\r\n.\r\n;").await?;
    f.deliver(MESSAGE, true, false).await?;
    assert_eq!(maild::smtp::queue::list(&f.db.conn, 10).await?.len(), 1);
    Ok(())
}
#[tokio::test]
async fn outbound_failure_after_keep_does_not_request_duplicate_delivery() -> Result<()> {
    let f = Fixture::new().await?;
    f.db.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_sieve_queue BEFORE INSERT ON smtp_queue BEGIN SELECT RAISE(FAIL, 'injected queue failure'); END;")?;
    f.script(r#"require "copy"; redirect :copy "outside@example.org";"#)
        .await?;
    assert!(f.deliver(MESSAGE, true, false).await?.is_some());
    assert_eq!(f.count("Inbox"), 1);
    f.script(r#"redirect "outside@example.org";"#).await?;
    assert!(f.deliver(MESSAGE, true, false).await.is_err());
    assert_eq!(f.count("Inbox"), 1);
    f.script(r#"require "vacation"; vacation "Away";"#).await?;
    assert!(f.deliver(MESSAGE, true, false).await?.is_some());
    assert_eq!(f.count("Inbox"), 2);
    assert_eq!(f.db.conn.lock().unwrap().query_row("SELECT count(*) FROM sieve_replies",[],|r| r.get::<_,i64>(0))?,0);
    f.db.conn.lock().unwrap().execute_batch("DROP TRIGGER fail_sieve_queue;")?;
    f.deliver(MESSAGE,true,false).await?;
    assert_eq!(maild::smtp::queue::list(&f.db.conn,10).await?.len(),1);
    Ok(())
}
#[test]
fn daemon_spam_verdict_replaces_forged_headers() {
    let bytes = delivery::stamp_spam(
        b"X-Mixos-Spam-Verdict: HAM\r\nSubject: test\r\nX-Mixos-Spam-Score: 0\r\n\r\nBody\r\n",
        "SPAM",
        0.99,
    );
    let plan = maild::sieve::compile(r#"if header :is "X-Mixos-Spam-Verdict" "HAM" { discard; }"#)
        .unwrap()
        .evaluate(maild::sieve::Context {
            data: &bytes,
            from: "a",
            to: "b",
        })
        .unwrap();
    assert_eq!(plan.filings.len(), 1);
    assert!(String::from_utf8(bytes).unwrap().ends_with("\r\nBody\r\n"));
}
