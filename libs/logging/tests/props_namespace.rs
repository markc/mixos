// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

#![cfg(feature = "props")]

//! Integration tests for `register` and the hook
//! enforcement points. These run against a real
//! `SqliteStore::new(...)` over an in-memory connection and a real
//! `PropsRouter`, so they exercise the same code paths a service
//! daemon will hit at boot.

use std::collections::BTreeMap;
use std::sync::Arc;

use logging::props::{
    INVALID_FILTER_PREFIX, UNDELETABLE_PREFIX, UNKNOWN_ENUM_VARIANT_PREFIX, register,
};
use props::bus::mutation::PropsRouter;
use props::record::{Actor, RecordKey, Version};
use props::runtime::{DeleteOpts, SetOpts};
use props::sqlite::SqliteStore;
use props::store::MergeMode;
use props::value::PropValue;
use rusqlite::Connection;

fn build_store(service: &str) -> Arc<SqliteStore> {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    Arc::new(SqliteStore::new(service, conn).expect("init store schema"))
}

fn singleton_key() -> RecordKey {
    RecordKey::singleton(logging::props::namespace_name())
}

fn set_opts(merge: MergeMode) -> SetOpts {
    SetOpts {
        expected_version: Some(Version::zero()),
        merge,
        actor: Actor::operator("test").expect("valid actor"),
        cause: None,
        ts_ms: 0,
    }
}

fn body(pairs: &[(&str, &str)]) -> PropValue {
    let mut m = BTreeMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), PropValue::String((*v).to_string()));
    }
    PropValue::Object(m)
}

#[test]
fn property_reloads_resynchronise_the_log_facade() {
    const CHILD: &str = "MIXOS_LOG_PROPS_FACADE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "property_reloads_resynchronise_the_log_facade", "--nocapture"])
            .env(CHILD, "1").env_remove("RUST_LOG").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("dependency debug admitted"), "{stderr}");
        assert!(!stderr.contains("dependency debug rejected"), "{stderr}");
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let guard = logging::init(
            &logging::LogOpts { log_level: Some(logging::LogLevel::Warn), log_stderr: Some(logging::TriState::Always), ..Default::default() },
            &logging::StatsOpts::default(), logging::LogDefaults::gui("fixture")
        ).unwrap();
        assert_eq!(log::max_level(), log::LevelFilter::Warn);
        let store = build_store("fixture");
        let mut router = PropsRouter::new("fixture");
        let namespace = register(&mut router, &store).unwrap();
        let runtime = namespace.runtime().clone();
        runtime.set(singleton_key(), body(&[("level", "debug")]), set_opts(MergeMode::Replace)).await.unwrap();
        logging::props::attach(&guard, namespace).await.unwrap();
        assert_eq!(log::max_level(), log::LevelFilter::Debug);
        log::debug!(target: "fake_dependency", "dependency debug admitted");
        for (level, expected) in [("trace", log::LevelFilter::Trace), ("warn", log::LevelFilter::Warn), ("none", log::LevelFilter::Off)] {
            let mut opts = set_opts(MergeMode::Replace);
            opts.expected_version = None;
            runtime.set(singleton_key(), body(&[("level", level)]), opts).await.unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while log::max_level() != expected {
                assert!(std::time::Instant::now() < deadline, "facade did not follow {level}");
                tokio::task::yield_now().await;
            }
            if level == "warn" {
                log::debug!(target: "fake_dependency", "dependency debug rejected");
            }
        }
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn registers_cleanly_against_propsrouter() {
    let store = build_store("demod");
    let mut router = PropsRouter::new("demod");
    let runtime = register(&mut router, &store)
        .expect("register log namespace once")
        .runtime()
        .clone();
    assert_eq!(runtime.spec().name.as_str(), "log");
}

#[tokio::test(flavor = "multi_thread")]
async fn re_registering_is_a_conflict() {
    let store = build_store("demod");
    let mut router = PropsRouter::new("demod");
    register(&mut router, &store).expect("first registration");
    let err = match register(&mut router, &store) {
        Ok(_) => panic!("second registration must conflict on the router"),
        Err(e) => e,
    };
    assert!(
        format!("{err}").contains("log"),
        "error should mention the namespace: {err}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_with_valid_payload_succeeds() {
    let store = build_store("demod");
    let mut router = PropsRouter::new("demod");
    let runtime = register(&mut router, &store).unwrap().runtime().clone();

    let value = body(&[
        ("level", "debug"),
        ("filter", "logging=debug"),
        ("format", "json"),
        ("applied_at", "2026-05-21T00:00:00Z"),
        ("applied_by", "props"),
        ("source", "debug,logging=debug"),
    ]);
    runtime
        .set(singleton_key(), value, set_opts(MergeMode::Replace))
        .await
        .expect("valid log row should commit");
}

#[tokio::test(flavor = "multi_thread")]
async fn set_with_malformed_filter_is_rejected() {
    let store = build_store("demod");
    let mut router = PropsRouter::new("demod");
    let runtime = register(&mut router, &store).unwrap().runtime().clone();

    let value = body(&[
        ("level", "info"),
        ("filter", "this is not a valid envfilter ::: ==="),
        ("format", "human"),
        ("applied_at", ""),
        ("applied_by", "default"),
        ("source", ""),
    ]);
    let err = runtime
        .set(singleton_key(), value, set_opts(MergeMode::Replace))
        .await
        .expect_err("malformed filter must be rejected by before_set");
    let msg = format!("{err}");
    assert!(
        msg.contains(INVALID_FILTER_PREFIX),
        "error should carry the invalid-filter prefix: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_with_unknown_level_is_rejected() {
    let store = build_store("demod");
    let mut router = PropsRouter::new("demod");
    let runtime = register(&mut router, &store).unwrap().runtime().clone();

    let value = body(&[
        ("level", "verbose"),
        ("filter", ""),
        ("format", "human"),
        ("applied_at", ""),
        ("applied_by", "default"),
        ("source", ""),
    ]);
    let err = runtime
        .set(singleton_key(), value, set_opts(MergeMode::Replace))
        .await
        .expect_err("unknown level variant must be rejected");
    let msg = format!("{err}");
    assert!(
        msg.contains(UNKNOWN_ENUM_VARIANT_PREFIX),
        "error should carry the unknown-enum prefix: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_is_refused() {
    let store = build_store("demod");
    let mut router = PropsRouter::new("demod");
    let runtime = register(&mut router, &store).unwrap().runtime().clone();

    // Materialise the row first so delete has something to address.
    runtime
        .set(
            singleton_key(),
            body(&[
                ("level", "info"),
                ("filter", ""),
                ("format", "human"),
                ("applied_at", ""),
                ("applied_by", "default"),
                ("source", ""),
            ]),
            set_opts(MergeMode::Replace),
        )
        .await
        .expect("seed row");

    let err = runtime
        .delete(
            singleton_key(),
            DeleteOpts {
                expected_version: None,
                actor: Actor::operator("test").expect("valid actor"),
                cause: None,
                ts_ms: 1,
            },
        )
        .await
        .expect_err("delete must be rejected on this singleton");
    let msg = format!("{err}");
    assert!(
        msg.contains(UNDELETABLE_PREFIX),
        "error should carry the undeletable prefix: {msg}"
    );
}
