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
            .body(json!({"using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail", "urn:ietf:params:jmap:submission"], "createdIds": {}, "methodCalls": calls}).to_string())
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
            "Received: from example.org by example.com; Mon, 1 Jan 2024 00:00:00 +0000\r\nDate: Mon, 1 Jan 2024 00:00:00 +0000\r\nFrom: alice@example.com\r\nTo: recipient@example.net\r\nSubject: {subject}\r\nMessage-ID: <{subject}@example.com>\r\n\r\nHello\r\n"
        );
        self.upload_message(&raw).await
    }

    async fn upload_message(&self, raw: &str) -> String {
        let response = self
            .client
            .post(format!("{}/jmap/upload/1", self.url))
            .basic_auth("alice@example.com", Some("password"))
            .header("Content-Type", "message/rfc822")
            .body(raw.to_owned())
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
        "duplicate": {"blobId": blob, "mailboxIds": {&f.inbox: true, f.inbox.replace('-', ""): true}},
        "keywords": {"blobId": blob, "mailboxIds": {&f.inbox: true}, "keywords": {"$seen": false}},
        "date": {"blobId": blob, "mailboxIds": {&f.inbox: true}, "receivedAt": "2024-01-01T10:00:00+10:00"}
    }})).await;
    for key in [
        "empty",
        "missing",
        "mailbox",
        "duplicate",
        "keywords",
        "date",
    ] {
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
    let maild::mailstore::BlobRefLookup::Found(hash) = f
        .built
        .app_state
        .mailstore
        .resolve_blob_ref(1, maild::mailstore::BlobId(a.parse().unwrap()))
        .unwrap()
    else {
        panic!("uploaded blob missing");
    };
    let size = f.built.app_state.mailstore.read_blob(hash).unwrap().len();
    f.built
        .app_state
        .db
        .conn
        .lock()
        .unwrap()
        .execute("UPDATE accounts SET quota = ?1 WHERE id = 1", [size as i64])
        .unwrap();
    let args =
        |blob: &str| json!({"emails": {"a": {"blobId": blob, "mailboxIds": {&f.inbox: true}}}});
    let (first, second) = tokio::join!(
        f.call("Email/import", args(&a)),
        f.call("Email/import", args(&b))
    );
    let (created, refused) = if first["created"].is_null() {
        (second, first)
    } else {
        (first, second)
    };
    assert!(created["created"]["a"]["id"].is_string(), "{created}");
    assert_eq!(refused["notCreated"]["a"]["type"], "overQuota", "{refused}");
    assert_eq!(
        f.call("Email/query", json!({})).await["ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn email_import_eai_and_foreign_blobs() {
    let f = Fixture::new().await;
    let raw = "From: Δοκιμή <δοκιμή@example.org>\r\nTo: recipient@example.net\r\nSubject: Unicode café\r\n\r\nΚαλημέρα\r\n";
    let blob = f.upload_message(raw).await;
    let response = f
        .call(
            "Email/import",
            json!({"emails": {"a": {"blobId": blob, "mailboxIds": {&f.inbox: true}}}}),
        )
        .await;
    let created = &response["created"]["a"];
    assert_eq!(created["size"], raw.len());
    let email = f.get(&created["id"]).await;
    assert_eq!(email["subject"], "Unicode café");
    let download = f
        .client
        .get(format!(
            "{}/jmap/blob/{}",
            f.url,
            created["blobId"].as_str().unwrap()
        ))
        .basic_auth("alice@example.com", Some("password"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(download, raw);
    let ms = &f.built.app_state.mailstore;
    ms.ensure_account_set(2).unwrap();
    let bytes = b"From: foreign@example.org\r\nSubject: foreign\r\n\r\nprivate";
    let hash = ms.put_blob(bytes).unwrap();
    let alias = ms
        .create_blob_ref(
            2,
            hash,
            bytes.len() as u64,
            chrono::Utc::now().timestamp_millis() + 3_600_000,
        )
        .unwrap();
    for id in [alias.0.to_string(), mds::blob::hex(&hash)] {
        let response = f
            .call(
                "Email/import",
                json!({"emails": {"a": {"blobId": id, "mailboxIds": {&f.inbox: true}}}}),
            )
            .await;
        assert_eq!(
            response["notCreated"]["a"]["type"], "invalidProperties",
            "{response}"
        );
        assert_eq!(response["oldState"], response["newState"]);
    }
    let empty = f.upload_message("").await;
    let invalid = f
        .call(
            "Email/import",
            json!({"emails": {"a": {"blobId": empty, "mailboxIds": {&f.inbox: true}}}}),
        )
        .await;
    assert_eq!(invalid["notCreated"]["a"]["type"], "invalidEmail");
}

#[tokio::test]
async fn email_import_and_submission_resolve_creation_ids_in_one_request() {
    let f = Fixture::new().await;
    let blob = f.upload("chain").await;
    let identities = f.call("Identity/get", json!({})).await;
    let response = f.request(json!([
        ["Mailbox/set", {"accountId": "1", "create": {"box": {"name": "Imported"}}}, "m"],
        ["Email/import", {"accountId": "1", "emails": {"draft": {"blobId": blob, "mailboxIds": {"#box": true}, "keywords": {"$draft": true}}}}, "i"],
        ["EmailSubmission/set", {"accountId": "1", "create": {"sent": {"emailId": "#draft", "identityId": identities["list"][0]["id"],
            "envelope": {"mailFrom": {"email": "alice@example.com"}, "rcptTo": [{"email": "recipient@example.net"}]}}}}, "s"]
    ])).await;
    let calls = &response["methodResponses"];
    assert!(
        calls[1][1]["created"]["draft"]["id"].is_string(),
        "{response}"
    );
    assert!(
        calls[2][1]["created"]["sent"]["id"].is_string(),
        "{response}"
    );
    assert_eq!(
        response["createdIds"]["draft"],
        calls[1][1]["created"]["draft"]["id"]
    );
    assert_eq!(
        response["createdIds"]["box"],
        calls[0][1]["created"]["box"]["id"]
    );
}

#[tokio::test]
async fn email_import_storage_failure_is_server_fail_and_rolls_back() {
    let f = Fixture::new().await;
    let blob = f.upload("storage-failure").await;
    let set = maild::mailstore::account_id_to_setid(1);
    let ms = &f.built.app_state.mailstore;
    ms.mds()
        .with_set_tx(&set, |tx| {
            tx.tx()
                .execute_batch(
                    "CREATE TRIGGER fail_import BEFORE INSERT ON mail_envelopes BEGIN SELECT RAISE(ABORT, 'fixture storage fault'); END;",
                )
                .map_err(|error| mds::Error::Other(error.to_string()))?;
            Ok(())
        })
        .unwrap();
    let response = f
        .call(
            "Email/import",
            json!({"emails": {"a": {"blobId": blob, "mailboxIds": {&f.inbox: true}}}}),
        )
        .await;
    assert_eq!(
        response["notCreated"]["a"]["type"], "serverFail",
        "{response}"
    );
    assert!(response["created"].is_null());
    assert_eq!(response["oldState"], response["newState"]);
    assert_eq!(f.call("Email/query", json!({})).await["ids"], json!([]));
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn email_import_parser_panic_is_invalid_email_and_service_survives() {
    let f = Fixture::new().await;
    // This existing attachment regression triggers mail-parser's debug-only
    // "Invalid part ID" assertion. Release builds parse the same bytes.
    let raw = "Content-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: message/rfc822\r\n\r\nContent-Type: message/rfc822\r\n\r\nContent-Type: application/octet-stream\r\nContent-Transfer-Encoding: base64\r\n\r\nAP8=\r\n--x--\r\n";
    let blob = f.upload_message(raw).await;
    let response = f
        .call(
            "Email/import",
            json!({"emails": {"a": {"blobId": blob, "mailboxIds": {&f.inbox: true}}}}),
        )
        .await;
    assert_eq!(
        response["notCreated"]["a"]["type"], "invalidEmail",
        "{response}"
    );
    assert!(response["created"].is_null());
    assert_eq!(response["oldState"], response["newState"]);
    assert!(f.import("after-parser-panic").await["created"]["a"]["id"].is_string());
}

#[tokio::test]
async fn email_set_invalid_mime_is_a_property_error() {
    let f = Fixture::new().await;
    let blob = f.upload_message("").await;
    let response = f
        .call(
            "Email/set",
            json!({"create": {"bad": {"blobId": blob, "mailboxIds": {&f.inbox: true}}}}),
        )
        .await;
    assert_eq!(
        response["notCreated"]["bad"]["type"], "invalidProperties",
        "{response}"
    );
    assert_eq!(
        response["notCreated"]["bad"]["properties"],
        json!(["blobId"])
    );
    assert_eq!(response["oldState"], response["newState"]);
}

#[tokio::test]
async fn email_import_null_received_at_uses_received_header_or_server_time() {
    let f = Fixture::new().await;
    let header = f.upload("null-date").await;
    let no_header = f
        .upload_message("Date: Mon, 1 Jan 2024 00:00:00 +0000\r\nFrom: alice@example.com\r\nSubject: no-received\r\n\r\nHello")
        .await;
    let before = chrono::Utc::now().timestamp_millis();
    let response = f.call("Email/import", json!({"emails": {
        "header": {"blobId": header, "mailboxIds": {&f.inbox: true}, "receivedAt": null},
        "clock": {"blobId": no_header, "mailboxIds": {&f.inbox: true}, "receivedAt": null},
        "offset": {"blobId": header, "mailboxIds": {&f.inbox: true}, "receivedAt": "2024-01-01T00:00:00+00:00"}
    }})).await;
    assert_eq!(
        f.get(&response["created"]["header"]["id"]).await["receivedAt"],
        "2024-01-01T00:00:00.000Z"
    );
    let clock = f.get(&response["created"]["clock"]["id"]).await;
    let clock = chrono::DateTime::parse_from_rfc3339(clock["receivedAt"].as_str().unwrap())
        .unwrap()
        .timestamp_millis();
    assert!(clock >= before && clock <= chrono::Utc::now().timestamp_millis());
    assert_eq!(
        response["notCreated"]["offset"]["type"],
        "invalidProperties"
    );
}

// Observe the real SQLite connection, including the executing OS thread. This
// catches accidental synchronous calls on a current-thread async HTTP server.
struct SqlTrace {
    context: Box<TraceContext>,
    ms: std::sync::Arc<maild::mailstore::SqliteMailStore>,
}
struct TraceContext {
    executor: std::thread::ThreadId,
    statements: std::sync::Mutex<Vec<(String, bool)>>,
}
unsafe extern "C" fn trace_statement(
    kind: u32,
    context: *mut std::ffi::c_void,
    statement: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
) -> i32 {
    if kind == rusqlite::ffi::SQLITE_TRACE_STMT {
        // SAFETY: SQLite retains the live boxed context until SqlTrace drops;
        // STMT events provide a sqlite3_stmt valid for this callback.
        let context = unsafe { &*(context.cast::<TraceContext>()) };
        let sql = unsafe { rusqlite::ffi::sqlite3_sql(statement.cast()) };
        if !sql.is_null() {
            let sql = unsafe { std::ffi::CStr::from_ptr(sql) }
                .to_string_lossy()
                .into_owned();
            if let Ok(mut statements) = context.statements.lock() {
                statements.push((sql, std::thread::current().id() == context.executor));
            }
        }
    }
    0
}
impl SqlTrace {
    fn new(f: &Fixture) -> Self {
        let mut trace = Self {
            context: Box::new(TraceContext {
                executor: std::thread::current().id(),
                statements: Default::default(),
            }),
            ms: f.built.app_state.mailstore.clone(),
        };
        let context = (&mut *trace.context as *mut TraceContext) as usize;
        trace
            .ms
            .mds()
            .with_set_read(&maild::mailstore::account_id_to_setid(1), |conn| {
                // SAFETY: the guard owns the context and removes the callback before
                // freeing it. with_set_read serialises access to this connection.
                let result = unsafe {
                    rusqlite::ffi::sqlite3_trace_v2(
                        conn.handle(),
                        rusqlite::ffi::SQLITE_TRACE_STMT,
                        Some(trace_statement),
                        context as *mut std::ffi::c_void,
                    )
                };
                assert_eq!(result, rusqlite::ffi::SQLITE_OK);
                Ok(())
            })
            .unwrap();
        trace
    }
    fn assert_off_executor(&self) {
        let statements = self.context.statements.lock().unwrap();
        assert!(!statements.is_empty());
        let blocking = statements
            .iter()
            .filter(|(_, on_executor)| *on_executor)
            .collect::<Vec<_>>();
        assert!(
            blocking.is_empty(),
            "SQLite executed on async executor: {blocking:?}"
        );
    }
}
impl Drop for SqlTrace {
    fn drop(&mut self) {
        self.ms
            .mds()
            .with_set_read(&maild::mailstore::account_id_to_setid(1), |conn| {
                // SAFETY: removing the callback while holding the connection lock
                // ensures no subsequent callback can use the context being freed.
                unsafe {
                    rusqlite::ffi::sqlite3_trace_v2(conn.handle(), 0, None, std::ptr::null_mut());
                }
                Ok(())
            })
            .unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn email_import_storage_runs_off_async_executor() {
    let f = Fixture::new().await;
    let blob = f.upload("import-thread").await;
    let trace = SqlTrace::new(&f);
    let response = f
        .call(
            "Email/import",
            json!({"emails": {"a": {"blobId": blob, "mailboxIds": {&f.inbox: true}}}}),
        )
        .await;
    assert!(response["created"]["a"]["id"].is_string(), "{response}");
    trace.assert_off_executor();
}

#[tokio::test(flavor = "current_thread")]
async fn email_set_blob_storage_runs_off_async_executor() {
    let f = Fixture::new().await;
    let blob = f.upload("set-thread").await;
    let trace = SqlTrace::new(&f);
    let response = f
        .call(
            "Email/set",
            json!({"create": {"a": {"blobId": blob, "mailboxIds": {&f.inbox: true}}}}),
        )
        .await;
    assert!(response["created"]["a"]["id"].is_string(), "{response}");
    trace.assert_off_executor();
}

#[tokio::test(flavor = "current_thread")]
async fn email_get_batches_optional_threads_off_async_executor_without_writes() {
    let f = Fixture::new().await;
    let a = f.import("thread-a").await["created"]["a"].clone();
    let b = f.import("thread-b").await["created"]["a"].clone();
    let ms = &f.built.app_state.mailstore;
    let set = maild::mailstore::account_id_to_setid(1);
    ms.mds()
        .with_set_tx(&set, |tx| {
            tx.tx()
                .execute(
                    "DELETE FROM mail_threads WHERE item_id = ?1",
                    [b["id"].as_str().unwrap()],
                )
                .map_err(|e| mds::Error::Other(e.to_string()))?;
            Ok(())
        })
        .unwrap();
    // A plain reader succeeds on a query-only connection. BEGIN IMMEDIATE from
    // with_set_tx would fail here even if it contained only SELECT statements.
    ms.mds()
        .with_set_read(&set, |conn| {
            conn.execute_batch("PRAGMA query_only=ON").unwrap();
            Ok(())
        })
        .unwrap();
    assert!(
        ms.thread_for_email(1, mds::ItemId(b["id"].as_str().unwrap().parse().unwrap()))
            .unwrap()
            .is_none()
    );
    ms.mds()
        .with_set_read(&set, |conn| {
            conn.execute_batch("PRAGMA query_only=OFF").unwrap();
            Ok(())
        })
        .unwrap();
    let trace = SqlTrace::new(&f);
    let response = f
        .call(
            "Email/get",
            json!({"ids": [a["id"], b["id"]], "properties": ["id", "threadId"]}),
        )
        .await;
    assert_eq!(response["list"][0]["threadId"], a["threadId"]);
    assert!(response["list"][1]["threadId"].is_null(), "{response}");
    trace.assert_off_executor();
    let statements = trace.context.statements.lock().unwrap();
    assert_eq!(
        statements
            .iter()
            .filter(|(sql, _)| sql.contains("FROM mail_threads"))
            .count(),
        1,
        "{statements:?}"
    );
}

#[tokio::test]
async fn duplicate_import_into_other_mailboxes_preserves_existing_email() {
    let f = Fixture::new().await;
    let first = f.import("different-memberships").await;
    let id = &first["created"]["a"]["id"];
    let before = f.get(id).await;
    let blob = f.upload("different-memberships").await;
    let response = f.call("Email/import", json!({"emails": {"b": {"blobId": blob, "mailboxIds": {&f.archive: true}, "keywords": {"$seen": true}, "receivedAt": "2025-01-01T00:00:00Z"}}})).await;
    assert_eq!(
        response["notCreated"]["b"],
        json!({"type": "alreadyExists", "existingId": id})
    );
    assert_eq!(response["oldState"], response["newState"]);
    assert_eq!(f.get(id).await, before);
}

#[tokio::test]
async fn creation_references_cover_same_call_parent_update_destroy_and_errors() {
    let f = Fixture::new().await;
    let blob = f.upload("references").await;
    let response = f.request(json!([
        ["Mailbox/set", {"accountId": "1", "create": {
            "a-child": {"name": "Child", "parentId": "#z-parent"},
            "z-parent": {"name": "Parent"}, "bad": {"name": "Bad", "parentId": "#absent"}
        }, "update": {"#a-child": {"name": "Renamed"}, "#absent": {"name": "No"}}, "destroy": ["#absent"]}, "m"],
        ["Email/set", {"accountId": "1", "create": {"email": {"blobId": blob, "mailboxIds": {"#a-child": true}}},
            "update": {"#email": {"keywords/$seen": true}, "#absent": {"keywords/$seen": true}}, "destroy": ["#absent"]}, "e"]
    ])).await;
    let calls = &response["methodResponses"];
    let child = &calls[0][1]["created"]["a-child"]["id"];
    assert!(child.is_string(), "{response}");
    assert!(
        calls[0][1]["updated"]
            .as_object()
            .unwrap()
            .contains_key(child.as_str().unwrap())
    );
    assert_eq!(
        calls[0][1]["notCreated"]["bad"]["type"],
        "invalidProperties"
    );
    assert_eq!(calls[0][1]["notUpdated"]["#absent"]["type"], "notFound");
    assert_eq!(calls[0][1]["notDestroyed"]["#absent"]["type"], "notFound");
    let id = &calls[1][1]["created"]["email"]["id"];
    assert_eq!(f.get(id).await["keywords"]["$seen"], true, "{response}");
    let boxes = f.call("Mailbox/get", json!({"ids": [child]})).await;
    assert_eq!(boxes["list"][0]["name"], "Renamed");
    assert_eq!(
        boxes["list"][0]["parentId"],
        calls[0][1]["created"]["z-parent"]["id"]
    );
    let destroyed = f
        .request(json!([
            ["Email/set", {"accountId": "1", "destroy": ["#email"]}, "d"],
        ]))
        .await;
    // Creation ids are request-local: a later request must reject the old ref.
    assert_eq!(
        destroyed["methodResponses"][0][1]["notDestroyed"]["#email"]["type"],
        "notFound"
    );
    let created_destroyed = f.request(json!([["Mailbox/set", {"accountId": "1", "create": {"temp": {"name": "Temporary"}}, "destroy": ["#temp"]}, "t"]])).await;
    assert_eq!(
        created_destroyed["methodResponses"][0][1]["destroyed"],
        json!([created_destroyed["createdIds"]["temp"]])
    );
}

#[tokio::test]
async fn creation_references_resolve_calendar_contact_and_all_set_target_ids() {
    let f = Fixture::new().await;
    let response = f.request(json!([
        ["Calendar/set", {"accountId": "1", "create": {"cal": {"name": "Calendar"}}, "update": {"#cal": {"name": "Renamed"}}, "destroy": ["#absent"]}, "c"],
        ["CalendarEvent/set", {"accountId": "1", "create": {"ev": {"calendarId": "#cal", "title": "#literal"}, "bad-ev": {"calendarId": "#absent"}}, "update": {"#ev": {"title": "Changed"}}}, "e"],
        ["AddressBook/set", {"accountId": "1", "create": {"book": {"name": "Book"}}, "update": {"#book": {"name": "Renamed"}}}, "b"],
        ["Contact/set", {"accountId": "1", "create": {"person": {"addressBookId": "#book", "fullName": "#literal"}, "bad-person": {"addressBookId": "#absent"}}, "update": {"#person": {"fullName": "Changed"}}}, "p"],
        ["CalendarEvent/set", {"accountId": "1", "destroy": ["#ev", "#absent"]}, "ed"],
        ["Contact/set", {"accountId": "1", "destroy": ["#person", "#absent"]}, "pd"],
        ["VacationResponse/set", {"accountId": "1", "update": {"#absent": {"isEnabled": true}}}, "v"]
    ])).await;
    for (index, creation) in [(0, "cal"), (1, "ev"), (2, "book"), (3, "person")] {
        let call = &response["methodResponses"][index][1];
        let id = call["created"][creation]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("{response}"));
        assert!(
            call["updated"].as_object().unwrap().contains_key(id),
            "{response}"
        );
    }
    assert_eq!(
        response["methodResponses"][1][1]["notCreated"]["bad-ev"]["properties"],
        json!(["calendarId"])
    );
    assert_eq!(
        response["methodResponses"][3][1]["notCreated"]["bad-person"]["properties"],
        json!(["addressBookId"])
    );
    for index in [0, 4, 5] {
        assert_eq!(
            response["methodResponses"][index][1]["notDestroyed"]["#absent"]["type"],
            "notFound"
        );
    }
    for (index, key) in [(4, "ev"), (5, "person")] {
        assert_eq!(
            response["methodResponses"][index][1]["destroyed"],
            json!([response["createdIds"][key]])
        );
    }
    assert_eq!(
        response["methodResponses"][6][1]["notUpdated"]["#absent"]["type"],
        "notFound"
    );
}

#[tokio::test]
async fn submission_success_references_modify_only_the_successful_submissions_email() {
    let f = Fixture::new().await;
    let blob = f.upload("success-actions").await;
    let identities = f.call("Identity/get", json!({})).await;
    let response = f.request(json!([
        ["Mailbox/set", {"accountId": "1", "create": {"destination": {"name": "Sent fixture"}}}, "m"],
        ["Email/import", {"accountId": "1", "emails": {"draft": {"blobId": blob, "mailboxIds": {&f.inbox: true}, "keywords": {"$draft": true}}}}, "i"],
        ["EmailSubmission/set", {"accountId": "1", "create": {
            "sent": {"emailId": "#draft", "identityId": identities["list"][0]["id"]},
            "bad": {"emailId": "#absent"}
        }, "onSuccessUpdateEmail": {"#sent": {"keywords/$draft": null, "keywords/$seen": true, "mailboxIds/#destination": true, format!("mailboxIds/{}", f.inbox): null}}, "onSuccessDestroyEmail": ["#bad"]}, "s"]
    ])).await;
    let calls = &response["methodResponses"];
    assert_eq!(calls[2][0], "EmailSubmission/set", "{response}");
    assert_eq!(
        calls[2][1]["notCreated"]["bad"]["type"],
        "invalidProperties"
    );
    assert_eq!(calls[3][0], "Email/set", "{response}");
    assert_eq!(calls[3][2], "s");
    assert!(
        calls[3][1]["updated"]
            .as_object()
            .is_some_and(|m| m.contains_key(response["createdIds"]["draft"].as_str().unwrap())),
        "{response}"
    );
    let got = f.get(&response["createdIds"]["draft"]).await;
    assert_eq!(got["keywords"]["$seen"], true);
    assert!(got["keywords"]["$draft"].is_null());
    assert_eq!(
        got["mailboxIds"][response["createdIds"]["destination"].as_str().unwrap()],
        true
    );
    let destroy = f.request(json!([
        ["EmailSubmission/set", {"accountId": "1", "create": {"sent": {"emailId": response["createdIds"]["draft"]}}, "onSuccessDestroyEmail": ["#sent"]}, "d"]
    ])).await;
    assert_eq!(destroy["methodResponses"][1][0], "Email/set", "{destroy}");
    assert_eq!(
        destroy["methodResponses"][1][1]["destroyed"],
        json!([response["createdIds"]["draft"]])
    );
}
