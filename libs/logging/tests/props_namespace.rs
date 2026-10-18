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
    let runtime = register(&mut router, &store)
        .unwrap()
        .runtime()
        .clone();

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
    let runtime = register(&mut router, &store)
        .unwrap()
        .runtime()
        .clone();

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
    let runtime = register(&mut router, &store)
        .unwrap()
        .runtime()
        .clone();

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
    let runtime = register(&mut router, &store)
        .unwrap()
        .runtime()
        .clone();

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

#[tokio::test(flavor = "multi_thread")]
async fn set_carries_the_service_prefix_in_capability_names() {
    // The capability set returned by `auth_policy("otherd")` must scope
    // to `<service>.log`. This is what stops a misconfigured peer
    // holding `props.write:demod.log` from silencing otherd's logging.
    let otherd_policy = logging::props::auth_policy("otherd");
    let caps = otherd_policy.resolve(&Default::default());
    let names: Vec<String> = caps.iter().map(|c| c.as_str().to_string()).collect();
    assert!(
        names.iter().any(|n| n == "props.read:otherd.log"),
        "expected props.read:otherd.log in {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "props.write:otherd.log"),
        "expected props.write:otherd.log in {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.contains("demod.log")),
        "otherd policy must not contain demod.log caps: {names:?}"
    );
}
