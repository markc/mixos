// SPDX-License-Identifier: MIT OR Apache-2.0
//! RFC 8620/8621 contracts through the production authenticated HTTP router.
use maild::{
    config::Config,
    mailstore::MailStore,
    runtime::{BuiltMaild, RuntimeOpts, build_runtime},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

struct Fixture {
    _dir: tempfile::TempDir,
    built: BuiltMaild,
    server: tokio::task::JoinHandle<()>,
    client: reqwest::Client,
    url: String,
    inbox: String,
    archive: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = |name| dir.path().join(name).to_str().unwrap().to_owned();
        let cfg = Config {
            database_path: path("mail.db"),
            blob_dir: path("blobs"),
            mds_dir: path("mds"),
            spam_db_dir: Some(path("spam")),
            rule_stats_dir: Some(path("rules")),
            smtp_inbound: None,
            ..Default::default()
        };
        let built = build_runtime(
            &cfg,
            RuntimeOpts {
                enable_bus: false,
                disable_outbound_delivery: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        use ::props::{
            record::{Actor, RecordKey, Version},
            runtime::SetOpts,
            store::MergeMode,
            value::PropValue,
        };
        let value = PropValue::Object(BTreeMap::from([
            (
                "email".into(),
                PropValue::String("alice@example.com".into()),
            ),
            (
                "password".into(),
                PropValue::String(bcrypt::hash("password", 4).unwrap()),
            ),
            ("name".into(), PropValue::String("Sender".into())),
            ("quota".into(), PropValue::Int(0)),
            ("spam_enabled".into(), PropValue::Bool(false)),
        ]));
        built
            .accounts_runtime()
            .set(
                RecordKey::collection(
                    maild::props::accounts::namespace_name(),
                    "alice@example.com",
                ),
                value,
                SetOpts {
                    expected_version: Some(Version::zero()),
                    merge: MergeMode::Replace,
                    actor: Actor::operator("jmap-test").unwrap(),
                    cause: None,
                    ts_ms: 0,
                },
            )
            .await
            .unwrap();
        let mailboxes = built.app_state.mailstore.list_mailboxes(1).unwrap();
        let inbox = mailboxes
            .iter()
            .find(|m| m.role == Some(maild::mailstore::MailboxRole::Inbox))
            .unwrap()
            .id
            .0
            .to_string();
        let archive = mailboxes
            .iter()
            .find(|m| m.role == Some(maild::mailstore::MailboxRole::Archive))
            .unwrap()
            .id
            .0
            .to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = built.router.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            _dir: dir,
            built,
            server,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
            url,
            inbox,
            archive,
        }
    }

    async fn request(&self, calls: Value) -> Value {
        let response = self.client.post(format!("{}/jmap", self.url))
            .basic_auth("alice@example.com", Some("password"))
            .header("Content-Type", "application/json")
            .body(json!({"using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail", "urn:ietf:params:jmap:submission"], "methodCalls": calls}).to_string())
            .send().await.unwrap().error_for_status().unwrap();
        serde_json::from_str(&response.text().await.unwrap()).unwrap()
    }

    async fn call(&self, method: &str, mut args: Value) -> Value {
        args["accountId"] = "1".into();
        let response = self.request(json!([[method, args, "a"]])).await;
        assert_eq!(response["methodResponses"][0][0], method, "{response}");
        response["methodResponses"][0][1].clone()
    }

    async fn upload(&self, subject: &str) -> String {
        let raw = format!(
            "Received: from example.org by example.com; Mon, 1 Jan 2024 00:00:00 +0000\r\nFrom: alice@example.com\r\nTo: recipient@example.net\r\nSubject: {subject}\r\nMessage-ID: <{subject}@example.com>\r\n\r\nHello\r\n"
        );
        let response = self
            .client
            .post(format!("{}/jmap/upload/1", self.url))
            .basic_auth("alice@example.com", Some("password"))
            .header("Content-Type", "message/rfc822")
            .body(raw)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let value: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
        value["blobId"].as_str().unwrap().into()
    }

    async fn import(&self, subject: &str) -> Value {
        let blob = self.upload(subject).await;
        self.call(
            "Email/import",
            json!({"emails": {"a": {"blobId": blob, "mailboxIds": {&self.inbox: true}}}}),
        )
        .await
    }

    async fn get(&self, id: &Value) -> Value {
        self.call("Email/get", json!({"ids": [id]})).await["list"][0].clone()
    }
}

#[tokio::test]
async fn email_set_contract_create_honours_multi_mailbox_ids() {
    let f = Fixture::new().await;
    let blob = f.upload("multi").await;
    // blobId is maild's retained legacy Email/set extension. Mailbox
    // membership and atomic failure semantics are the RFC contract here.
    let response = f.call("Email/set", json!({"create": {
        "a": {"blobId": blob, "mailboxIds": {&f.inbox: true, &f.archive: true}},
        "b": {"blobId": blob, "mailboxIds": {&f.inbox: true, uuid::Uuid::nil().to_string(): true}}
    }})).await;
    let email = f.get(&response["created"]["a"]["id"]).await;
    assert_eq!(
        email["mailboxIds"],
        json!({&f.inbox: true, &f.archive: true})
    );
    assert_eq!(response["notCreated"]["b"]["type"], "invalidProperties");
    let query = f.call("Email/query", json!({})).await;
    assert_eq!(query["ids"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn email_set_contract_create_rejects_empty_mailbox_ids() {
    let f = Fixture::new().await;
    let blob = f.upload("empty").await;
    let response = f
        .call(
            "Email/set",
            json!({"create": {"a": {"blobId": blob, "mailboxIds": {}}}}),
        )
        .await;
    assert_eq!(response["notCreated"]["a"]["type"], "invalidProperties");
    assert!(
        response["notCreated"]["a"]["description"]
            .as_str()
            .unwrap()
            .contains("mailboxIds")
    );
    assert_eq!(response["oldState"], response["newState"]);
    assert_eq!(f.call("Email/query", json!({})).await["ids"], json!([]));
}

#[tokio::test]
async fn email_set_contract_update_partial_success_per_id() {
    let f = Fixture::new().await;
    let a = f.import("partial-a").await["created"]["a"]["id"].clone();
    let c = f.import("partial-c").await["created"]["a"]["id"].clone();
    // Sort the unknown id between both real ids so an early exit would
    // leave the later valid update unapplied.
    let mut ids = [a.as_str().unwrap(), c.as_str().unwrap()];
    ids.sort();
    let mut missing = ids[0].to_owned();
    missing.replace_range(24..36, "ffffffffffff");
    assert!(ids[0] < missing.as_str() && missing.as_str() < ids[1]);
    let response = f.call("Email/set", json!({"update": {
        ids[0]: {"keywords/$seen": true}, missing.clone(): {"keywords/$seen": true}, ids[1]: {"keywords/$flagged": true}
    }})).await;
    assert_eq!(response["notUpdated"][&missing]["type"], "notFound");
    assert_eq!(response["updated"].as_object().unwrap().len(), 2);
    assert_eq!(f.get(&json!(ids[0])).await["keywords"]["$seen"], true);
    assert_eq!(f.get(&json!(ids[1])).await["keywords"]["$flagged"], true);
}

#[tokio::test]
async fn email_set_contract_destroy_runs_after_update() {
    let f = Fixture::new().await;
    let id = f.import("ordering").await["created"]["a"]["id"].clone();
    let response = f
        .call(
            "Email/set",
            json!({"update": {id.as_str().unwrap(): {"keywords/$seen": true}}, "destroy": [id]}),
        )
        .await;
    assert_eq!(response["updated"], json!({id.as_str().unwrap(): null}));
    assert_eq!(response["destroyed"], json!([id]));
    let changes = f
        .call("Email/changes", json!({"sinceState": response["oldState"]}))
        .await;
    assert_eq!(changes["destroyed"], json!([id]));
    assert_eq!(changes["updated"], json!([]));
    assert_eq!(
        f.call("Email/get", json!({"ids": [id]})).await["notFound"],
        json!([id])
    );
}

#[tokio::test]
async fn email_set_contract_import_is_idempotent_on_blob_hash() {
    let f = Fixture::new().await;
    let first = f.import("duplicate").await;
    let id = &first["created"]["a"]["id"];
    // Upload the same bytes again to get a different alias for the same hash.
    let second = f.import("duplicate").await;
    assert_eq!(
        second["notCreated"]["a"],
        json!({"type": "alreadyExists", "existingId": id})
    );
    assert!(second["created"].is_null());
    assert_eq!(second["oldState"], second["newState"]);
    let got = f.get(id).await;
    assert_eq!(got["receivedAt"], "2024-01-01T00:00:00.000Z");
    assert_eq!(first["created"]["a"]["blobId"], got["blobId"]);
    assert!(first["created"]["a"]["threadId"].is_string());
    assert_eq!(first["created"]["a"]["threadId"], got["threadId"]);
    let changes = f
        .call("Email/changes", json!({"sinceState": first["oldState"]}))
        .await;
    assert_eq!(changes["created"], json!([id]));
}

#[tokio::test]
async fn email_import_validation_state_and_partial_success() {
    let f = Fixture::new().await;
    let blob = f.upload("validation").await;
    let response = f.call("Email/import", json!({"emails": {
        "a": {"blobId": blob, "mailboxIds": {&f.inbox: true}, "keywords": {"$draft": true}, "receivedAt": "2024-02-01T00:00:00Z"},
        "empty": {"blobId": blob, "mailboxIds": {}},
        "missing": {"blobId": uuid::Uuid::nil().to_string(), "mailboxIds": {&f.inbox: true}},
        "mailbox": {"blobId": blob, "mailboxIds": {uuid::Uuid::nil().to_string(): true}},
        "keywords": {"blobId": blob, "mailboxIds": {&f.inbox: true}, "keywords": {"$seen": false}},
        "date": {"blobId": blob, "mailboxIds": {&f.inbox: true}, "receivedAt": "2024-01-01T10:00:00+10:00"}
    }})).await;
    for key in ["empty", "missing", "mailbox", "keywords", "date"] {
        assert_eq!(
            response["notCreated"][key]["type"], "invalidProperties",
            "{response}"
        );
    }
    let got = f.get(&response["created"]["a"]["id"]).await;
    assert_eq!(got["keywords"]["$draft"], true);
    assert_eq!(got["receivedAt"], "2024-02-01T00:00:00.000Z");
    let stale = f.request(json!([["Email/import", {"accountId": "1", "ifInState": response["oldState"], "emails": {}}, "x"]])).await;
    assert_eq!(stale["methodResponses"][0][0], "error");
    assert_eq!(stale["methodResponses"][0][1]["type"], "stateMismatch");
    for args in [
        json!({"accountId": "1"}),
        json!({"accountId": "1", "emails": []}),
        json!({"accountId": "1", "ifInState": 7, "emails": {}}),
    ] {
        assert_eq!(
            f.request(json!([["Email/import", args, "x"]])).await["methodResponses"][0][1]["type"],
            "invalidArguments"
        );
    }
    // Valid CAS ids are account-owned blob references too; a repeat produces
    // alreadyExists, while a foreign account's CAS bytes stay inaccessible.
    let cas = f.call("Email/import", json!({"ifInState": response["newState"], "emails": {"a": {"blobId": got["blobId"], "mailboxIds": {&f.inbox: true}}}})).await;
    assert_eq!(cas["notCreated"]["a"]["type"], "alreadyExists");
    let unknown = f
        .call(
            "Email/import",
            json!({"emails": {"a": {"blobId": "0".repeat(64), "mailboxIds": {&f.inbox: true}}}}),
        )
        .await;
    assert_eq!(unknown["notCreated"]["a"]["type"], "invalidProperties");
    let _ = &f.built;
}

#[tokio::test]
async fn email_import_concurrent_duplicate_and_standard_submission() {
    let f = Fixture::new().await;
    let blob = f.upload("sending").await;
    let args = json!({"emails": {"a": {"blobId": blob, "mailboxIds": {&f.inbox: true}, "keywords": {"$draft": true}}}});
    let (a, b) = tokio::join!(
        f.call("Email/import", args.clone()),
        f.call("Email/import", args)
    );
    let (created, duplicate) = if a["created"].is_null() {
        (b, a)
    } else {
        (a, b)
    };
    let id = &created["created"]["a"]["id"];
    assert_eq!(duplicate["notCreated"]["a"]["existingId"], *id);
    let identities = f.call("Identity/get", json!({})).await;
    let submission = f.call("EmailSubmission/set", json!({"create": {"s": {
        "emailId": id, "identityId": identities["list"][0]["id"],
        "envelope": {"mailFrom": {"email": "alice@example.com"}, "rcptTo": [{"email": "recipient@example.net"}]}
    }}})).await;
    assert!(submission["created"]["s"]["id"].is_string(), "{submission}");
}

#[tokio::test]
async fn email_import_quota_is_atomic_and_does_not_count_upload_staging() {
    let f = Fixture::new().await;
    let a = f.upload("quota-a").await;
    let b = f.upload("quota-b").await;
    let maild::mailstore::BlobRefLookup::Found(hash) = f.built.app_state.mailstore.resolve_blob_ref(1, maild::mailstore::BlobId(a.parse().unwrap())).unwrap() else { panic!("uploaded blob missing"); };
    let size = f.built.app_state.mailstore.read_blob(hash).unwrap().len();
    f.built.app_state.db.conn.lock().unwrap().execute("UPDATE accounts SET quota = ?1 WHERE id = 1", [size as i64]).unwrap();
    let args = |blob: &str| json!({"emails": {"a": {"blobId": blob, "mailboxIds": {&f.inbox: true}}}});
    let (first, second) = tokio::join!(f.call("Email/import", args(&a)), f.call("Email/import", args(&b)));
    let (created, refused) = if first["created"].is_null() { (second, first) } else { (first, second) };
    assert!(created["created"]["a"]["id"].is_string(), "{created}");
    assert_eq!(refused["notCreated"]["a"]["type"], "overQuota", "{refused}");
    assert_eq!(f.call("Email/query", json!({})).await["ids"].as_array().unwrap().len(), 1);
}
