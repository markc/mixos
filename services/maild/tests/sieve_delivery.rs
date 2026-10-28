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
        let mut db = Db::connect(
            tmp.path().join("mail.db").to_str().unwrap(),
            tmp.path().join("blobs").to_str().unwrap(),
        )
        .await?;
        db.sieve_hostname = Arc::new("mail.example.com".into());
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
        self.deliver_to(bytes, external, smtp, "bob@example.com").await
    }
    async fn deliver_to(&self, bytes: &[u8], external: bool, smtp: bool, to: &str) -> Result<Option<mds::ItemId>> {
        let domains = ["example.com".to_string()];
        delivery::deliver(
            &self.db,
            &self.ms,
            Delivery {
                account: &self.account,
                from: "sender@example.org",
                to,
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
async fn forged_hop_is_ignored_and_exhausted_received_drops_only_redirects() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["fileinto","mailbox","vacation"]; fileinto :create "Filed"; redirect "alice@example.com"; vacation "Away";"#).await?;
    let mut bytes = b"X-Mixos-Sieve-Hop: 999\r\n".to_vec();
    bytes.extend_from_slice(MESSAGE);
    f.deliver(&bytes, false, false).await?;
    assert_eq!(f.count("Filed"), 1);
    let alice_inbox = f.ms.mailbox_by_role(2, MailboxRole::Inbox)?.unwrap();
    assert_eq!(f.ms.list_emails_in_mailbox(2, alice_inbox, ListOpts::default())?.len(), 1);
    let f = Fixture::new().await?;
    f.script(r#"require ["fileinto","mailbox","vacation"]; fileinto :create "Filed"; redirect "alice@example.com"; vacation "Away";"#).await?;
    let mut bytes = "Received: from remote.example.org\r\n\tby MAIL.EXAMPLE.COM with ESMTP; date\r\n".repeat(maild::sieve::MAX_REDIRECTS).into_bytes();
    bytes.extend_from_slice(MESSAGE);
    f.deliver(&bytes, false, false).await?;
    assert_eq!(f.count("Filed"), 1, "loop suppression must preserve fileinto");
    let alice_inbox = f.ms.mailbox_by_role(2, MailboxRole::Inbox)?.unwrap();
    assert!(f.ms.list_emails_in_mailbox(2, alice_inbox, ListOpts::default())?.is_empty());
    assert_eq!(maild::smtp::queue::list(&f.db.conn, 10).await?.len(), 1, "vacation must survive redirect suppression");
    Ok(())
}

#[tokio::test]
async fn received_limit_restores_redirect_only_keep_and_ignores_other_hosts() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"redirect "alice@example.com";"#).await?;
    let mut bytes = "Received: from remote.example.org by mail.example.com; date\r\n".repeat(maild::sieve::MAX_REDIRECTS).into_bytes();
    bytes.extend_from_slice(MESSAGE);
    f.deliver(&bytes, false, false).await?;
    assert_eq!(f.count("Inbox"), 1);
    let mut bytes = "Received: from remote.example.org by mail.example.com.other.example.net; date\r\n".repeat(10).into_bytes();
    bytes.extend_from_slice(MESSAGE);
    f.deliver(&bytes, false, false).await?;
    let alice_inbox = f.ms.mailbox_by_role(2, MailboxRole::Inbox)?.unwrap();
    assert_eq!(f.ms.list_emails_in_mailbox(2, alice_inbox, ListOpts::default())?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn local_redirect_budget_preserves_each_recipients_filing_plan() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["fileinto","mailbox"]; fileinto :create "BobFiled"; redirect "alice@example.com"; redirect "bob@example.com";"#).await?;
    store::put(&f.db, 2, "main", r#"require ["fileinto","mailbox"]; fileinto :create "AliceFiled"; redirect "bob@example.com";"#, None).await?;
    store::activate(&f.db, 2, "main").await?;
    f.deliver(MESSAGE, false, false).await?;
    assert_eq!(f.count("BobFiled"), 3);
    assert_eq!(f.count("Inbox"), 0);
    let alice_folder = f.ms.mailbox_by_name(2, "AliceFiled")?.unwrap();
    assert_eq!(f.ms.list_emails_in_mailbox(2, alice_folder, ListOpts::default())?.len(), 2);
    let alice_inbox = f.ms.mailbox_by_role(2, MailboxRole::Inbox)?.unwrap();
    assert!(f.ms.list_emails_in_mailbox(2, alice_inbox, ListOpts::default())?.is_empty());
    Ok(())
}

#[tokio::test]
async fn offsite_redirect_strips_private_headers_and_adds_received_trace() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"redirect "remote@example.net";"#).await?;
    let mut bytes = b"X-Mixos-Sieve-Hop: 1\r\nX-Mixos-Spam-Verdict: HAM\r\nX-Mixos-Spam-Score: 0\r\nX-Mixos-Spam-Debug: private\r\n\tcontinued private\r\nReceived: from old.example.org by old.example.net; date\r\n".to_vec();
    bytes.extend_from_slice(MESSAGE);
    f.deliver(&bytes, true, false).await?;
    let queue = maild::smtp::queue::list(&f.db.conn, 10).await?;
    let bytes = f.ms.mds().get_blob(&queue[0].blob_hash.unwrap())?;
    let raw = String::from_utf8_lossy(&bytes);
    assert!(!raw.to_ascii_lowercase().contains("x-mixos-"));
    assert!(!raw.contains("continued private"));
    let parsed = mail_parser::MessageParser::default().parse(&bytes).unwrap();
    assert_eq!(parsed.header_values("Received").count(), 2);
    assert!(raw.contains("by mail.example.com"));
    assert!(raw.contains("by old.example.net"));
    Ok(())
}

#[tokio::test]
async fn fileinto_agrees_with_mailboxexists_on_insert_permission() -> Result<()> {
    for create in [false, true] {
        let f = Fixture::new().await?;
        f.ms.create_mailbox(1, "ReadOnly", None, mds::ContainerAttrs {special_use:None,subscribed:false,extra:serde_json::json!({"jmap_my_rights":{"may_add_items":false}})})?;
        f.script(&format!(r#"require ["fileinto","mailbox"]; if not mailboxexists "ReadOnly" {{ fileinto {} "ReadOnly"; }}"#, if create {":create"} else {""})).await?;
        f.deliver(MESSAGE, false, false).await?;
        assert_eq!(f.count("ReadOnly"), 0);
        assert_eq!(f.count("Inbox"), 1);
    }
    Ok(())
}

#[tokio::test]
async fn vacation_external_policy_is_independent_of_redirect_allowlist() -> Result<()> {
    for enabled in [true, false] {
        let mut f = Fixture::new().await?;
        assert!(f.db.sieve_vacation_external);
        f.db.sieve_vacation_external = enabled;
        f.script(r#"require "vacation"; vacation "Away";"#).await?;
        f.deliver(MESSAGE, false, false).await?;
        assert_eq!(maild::smtp::queue::list(&f.db.conn, 10).await?.len(), usize::from(enabled));
        f.script(r#"redirect "remote@example.net";"#).await?;
        f.deliver(MESSAGE, false, false).await?;
        assert_eq!(f.count("Inbox"), 2, "redirect still defaults to denied");
    }
    Ok(())
}

#[tokio::test]
async fn vacation_default_subject_decodes_and_reencodes_original_subject() -> Result<()> {
    for (header, expected, encoded) in [("Subject: Original\r\n", "Auto: Original", false),("Subject: =?UTF-8?Q?caf=C3=A9?=\r\n", "Auto: café", true),("Subject: =?UTF-8?Q?ASCII?=\r\n", "Auto: ASCII", false),("", "Automated reply", false)] {
        let f = Fixture::new().await?;
        f.script(r#"require "vacation"; vacation "Away";"#).await?;
        let data = format!("From: sender@example.org\r\nTo: bob@example.com\r\n{header}\r\nbody");
        f.deliver(data.as_bytes(), false, false).await?;
        let queue = maild::smtp::queue::list(&f.db.conn, 10).await?;
        let bytes = f.ms.mds().get_blob(&queue[0].blob_hash.unwrap())?;
        let parsed = mail_parser::MessageParser::default().parse(&bytes).unwrap();
        assert_eq!(parsed.subject(), Some(expected));
        let raw = String::from_utf8_lossy(&bytes);
        assert_eq!(raw.contains("=?"), encoded);
    }
    Ok(())
}

#[tokio::test]
async fn vacation_recognises_envelope_identity_in_all_recipient_headers() -> Result<()> {
    for field in ["To", "Cc", "Bcc", "Resent-To", "Resent-Cc", "Resent-Bcc"] {
        let f = Fixture::new().await?;
        f.script(r#"require "vacation"; vacation "Away";"#).await?;
        let data = format!("From: sender@example.org\r\n{field}: alias@example.com\r\n\r\nbody");
        f.deliver_to(data.as_bytes(), false, false, "alias@example.com").await?;
        assert_eq!(maild::smtp::queue::list(&f.db.conn, 10).await?.len(), 1, "{field}");
    }
    Ok(())
}

#[tokio::test]
async fn redirect_rejects_a_second_at_sign_in_the_local_part() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["fileinto","mailbox"]; fileinto :create "Filed"; redirect "bad@local@example.net";"#).await?;
    f.deliver(MESSAGE, true, false).await?;
    assert_eq!(f.count("Filed"), 0);
    assert_eq!(f.count("Inbox"), 1);
    assert!(maild::smtp::queue::list(&f.db.conn, 10).await?.is_empty());
    Ok(())
}

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
async fn repeated_filings_use_last_flags_and_keywords() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["imap4flags","fileinto"]; keep :flags ["\\Seen","first"]; fileinto :flags ["\\Flagged","second"] "INBOX";"#).await?;
    f.deliver(MESSAGE, false, false).await?;
    assert_eq!(f.count("Inbox"), 1);
    let inbox =
        f.ms.mailbox_by_role(f.account.id, MailboxRole::Inbox)?
            .unwrap();
    let items =
        f.ms.list_emails_in_mailbox(f.account.id, inbox, ListOpts::default())?;
    let record = f.ms.get_email(f.account.id, items[0].id)?;
    assert_eq!(
        record.keywords.0 & (Flags::SEEN | Flags::FLAGGED),
        Flags::FLAGGED
    );
    assert!(!record.tags.contains("first"));
    assert!(record.tags.contains("second"));
    f.script(r#"require ["imap4flags","fileinto"]; keep :flags "\\Seen"; fileinto "INBOX";"#)
        .await?;
    let id = f.deliver(MESSAGE, false, false).await?.unwrap();
    assert_eq!(f.ms.get_email(f.account.id, id)?.keywords.0, 0);
    Ok(())
}
#[tokio::test]
async fn unsupported_flags_are_ignored_without_changing_filing() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["imap4flags","fileinto","mailbox"]; fileinto :create :flags ["\\Seen", "\\Recent", "\\Unknown", "bad(flag)", "project"] "FlagTest";"#).await?;
    let id = f.deliver(MESSAGE, false, false).await?.unwrap();
    assert_eq!(f.count("Inbox"), 0);
    assert_eq!(f.count("FlagTest"), 1);
    let record = f.ms.get_email(f.account.id, id)?;
    assert_eq!(record.keywords.0, Flags::SEEN);
    assert_eq!(record.tags, Tags::from(vec!["project".into()]));
    let keywords = (0..70)
        .map(|n| format!("tag{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    f.script(&format!(
        r#"require ["imap4flags","fileinto"]; setflag "{keywords}"; fileinto "FlagTest";"#
    ))
    .await?;
    let id = f.deliver(MESSAGE, false, false).await?.unwrap();
    assert_eq!(f.count("Inbox"), 0);
    assert_eq!(
        f.ms.get_email(f.account.id, id)?.tags.len(),
        maild::keyword::MAX_KEYWORDS_PER_MESSAGE
    );
    Ok(())
}
#[tokio::test]
async fn mailboxexists_resolves_nested_account_owned_writable_mailboxes() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["fileinto","mailbox"]; fileinto :create "INBOX/Projects";"#)
        .await?;
    f.deliver(MESSAGE, false, false).await?;
    store::put(
        &f.db,
        2,
        "other",
        r#"require ["fileinto","mailbox"]; fileinto :create "OtherAccount";"#,
        None,
    )
    .await?;
    store::activate(&f.db, 2, "other").await?;
    f.script(r#"redirect "alice@example.com";"#).await?;
    f.deliver(MESSAGE, false, false).await?;
    f.ms.create_mailbox(
        f.account.id,
        "ReadOnly",
        None,
        mds::ContainerAttrs {
            special_use: None,
            subscribed: false,
            extra: serde_json::json!({"jmap_my_rights":{"may_add_items":false}}),
        },
    )?;
    f.script(r#"require "mailbox"; if allof(mailboxexists ["inbox","INBOX/Projects"],not mailboxexists "OtherAccount",not mailboxexists "ReadOnly") { discard; }"#).await?;
    assert!(f.deliver(MESSAGE, false, false).await?.is_none());
    assert_eq!(f.count("Inbox"), 0);
    Ok(())
}
#[tokio::test]
async fn mailbox_snapshot_work_is_bounded_and_skipped_when_unused() -> Result<()> {
    let f = Fixture::new().await?;
    let mut parent = None;
    for n in 0..160 {
        parent = Some(f.ms.create_mailbox(
            f.account.id,
            &format!("d{n}"),
            parent,
            mds::ContainerAttrs {
                special_use: None,
                subscribed: false,
                extra: serde_json::json!({}),
            },
        )?);
    }
    f.script(r#"require "mailbox"; if mailboxexists "INBOX" { discard; }"#)
        .await?;
    assert!(f.deliver(MESSAGE, false, false).await?.is_some());
    assert_eq!(f.count("Inbox"), 1);
    f.script("discard;").await?;
    assert!(f.deliver(MESSAGE, false, false).await?.is_none());
    assert_eq!(f.count("Inbox"), 1);
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
async fn created_mailboxes_and_parents_satisfy_later_filing_actions() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["fileinto","mailbox","copy","imap4flags"]; fileinto :create :copy "NewParent/Child"; fileinto :copy "NewParent"; fileinto :flags "\\Seen" "NewParent/Child";"#).await?;
    f.deliver(MESSAGE, false, false).await?;
    assert_eq!(f.count("Inbox"), 0);
    let parent = f.ms.mailbox_by_name(f.account.id, "NewParent")?.unwrap();
    assert_eq!(
        f.ms.list_emails_in_mailbox(f.account.id, parent, ListOpts::default())?
            .len(),
        1
    );
    let child =
        f.ms.list_mailboxes(f.account.id)?
            .into_iter()
            .find(|entry| entry.parent == Some(parent) && entry.name == "Child")
            .unwrap();
    let items =
        f.ms.list_emails_in_mailbox(f.account.id, child.id, ListOpts::default())?;
    assert_eq!(items.len(), 1);
    assert_ne!(
        f.ms.get_email(f.account.id, items[0].id)?.keywords.0 & Flags::SEEN,
        0
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
    let mut f = Fixture::new().await?;
    f.script(r#"require "vacation"; vacation "Away";"#).await?;
    f.db.sieve_vacation_external = false;
    f.deliver(MESSAGE, false, false).await?;
    f.db.sieve_vacation_external = true;
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
async fn native_jmap_submission_replaces_a_cotenants_forged_spam_verdict() -> Result<()> {
    let f = Fixture::new().await?;
    f.script(r#"require ["fileinto","mailbox"]; if allof(header :is "X-Mixos-Spam-Verdict" "UNKNOWN",header :is "X-Mixos-Spam-Score" "0.0000") { fileinto :create "TrustedSubmission"; } else { discard; }"#).await?;
    let bytes = b"From: alice@example.com\r\nTo: bob@example.com\r\nSubject: local\r\nX-Mixos-Spam-Verdict: HAM\r\nX-Mixos-Spam-Verdict: SPAM\r\nX-Mixos-Spam-Score: 99\r\n\r\nbody";
    let hash = f.ms.mds().put_blob(bytes)?;
    let parsed = mail_parser::MessageParser::default().parse(bytes).unwrap();
    let drafts = f.ms.mailbox_by_role(2, MailboxRole::Drafts)?.unwrap();
    let (item, _) = f.ms.create_email(2, drafts, hash, maild::jmap::email::build_envelope(&parsed), &None, Flags(0), Tags::new(), chrono::Utc::now().timestamp_millis())?;
    let aliases = Arc::new(::props::Runtime::new("maild", maild::props::aliases::spec(::props::Hooks::noop()), Arc::new(::props::MemoryStore::new("maild"))));
    let result = maild::jmap::submission::set(&f.db, &f.ms, &aliases, 2, serde_json::json!({"create":{"send":{"emailId":item.0.to_string()}}})).await?;
    assert!(result["created"]["send"]["id"].is_string(), "{result}");
    assert_eq!(f.count("TrustedSubmission"), 1);
    let mailbox = f.ms.mailbox_by_name(1, "TrustedSubmission")?.unwrap();
    let records = f.ms.list_emails_in_mailbox(1, mailbox, ListOpts::default())?;
    let bytes = f.ms.mds().get_blob(&records[0].blob_hash.unwrap())?;
    let parsed = mail_parser::MessageParser::default().parse(&bytes).unwrap();
    assert_eq!(parsed.header_values("X-Mixos-Spam-Verdict").count(), 1);
    assert_eq!(parsed.header("X-Mixos-Spam-Verdict").and_then(|h| h.as_text()), Some("UNKNOWN"));
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
    assert_eq!(
        f.db.conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM sieve_replies", [], |r| r
                .get::<_, i64>(0))?,
        0
    );
    f.db.conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_sieve_queue;")?;
    f.deliver(MESSAGE, true, false).await?;
    assert_eq!(maild::smtp::queue::list(&f.db.conn, 10).await?.len(), 1);
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
