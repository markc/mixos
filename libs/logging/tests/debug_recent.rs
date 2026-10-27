// SPDX-License-Identifier: MIT OR Apache-2.0
use serde_json::{Value, json};

fn call(suffix: &str, body: Value) -> Value {
    let (rc, answer) = logging::debug::request(
        "fixture",
        &format!("fixture.debug{suffix}"),
        &body.to_string(),
    )
    .unwrap();
    assert_eq!(rc, 0, "{answer}");
    answer
}

#[test]
fn recent_fields_paging_filter_limits_and_release() {
    let _guard = logging::debug::init("fixture", "warn", &mut vec!["fixture".into()]).unwrap();
    assert_eq!(call(".recent", json!({}))["events"], json!([]));
    call(".set", json!({"level":"debug"}));
    let cursor = call(".recent", json!({}))["seq"].as_u64().unwrap();
    for id in 0..5 {
        tracing::debug!(target: "logging", window_id = id, generation = 7, app_id = "example.app", success = true, "window placed");
    }
    tracing::info!(target: "logging", command = "fixture.drive", "command completed");
    let first = call(".recent", json!({"since":cursor,"limit":2}));
    assert_eq!(first["events"].as_array().unwrap().len(), 2);
    assert_eq!(
        first["events"][0]["fields"],
        json!({"window_id":0,"generation":7,"app_id":"example.app","success":true})
    );
    assert_eq!(first["events"][0]["message"], "window placed");
    assert_eq!(first["events"][0]["level"], "debug");
    assert!(first["events"][0]["ts"].as_str().unwrap().ends_with('Z'));
    let second = call(".recent", json!({"since":first["next"],"limit":2}));
    assert_eq!(second["events"][0]["fields"]["window_id"], 2);
    assert_eq!(second["seq"], first["seq"]);
    assert_eq!(
        call(".recent", json!({"since":cursor,"level":"info"}))["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(call(".recent", json!({"level":"off"}))["events"], json!([]));
    assert_eq!(
        call(".recent", json!({"limit":1}))["events"][0]["message"],
        "command completed"
    );
    for body in [
        json!({"limit":1001}),
        json!({"limit":0}),
        json!({"since":-1}),
        json!({"level":"bad"}),
        json!({"unknown":true}),
        json!(null),
    ] {
        assert_eq!(
            logging::debug::request("fixture", "fixture.debug.recent", &body.to_string())
                .unwrap()
                .0,
            10
        );
    }
    for id in 0..400 {
        tracing::debug!(target: "logging", window_id = id, "page test");
    }
    assert_eq!(
        call(".recent", json!({}))["events"]
            .as_array()
            .unwrap()
            .len(),
        200
    );
    assert!(
        call(".recent", json!({"since": 0, "limit": 1000}))["events"]
            .as_array()
            .unwrap()
            .len()
            <= 1000
    );
    for level in ["warn", "off", "info", "warn"] {
        call(".set", json!({"level":level}));
        assert_eq!(call(".recent", json!({}))["events"], json!([]));
    }
    call(".set", json!({"level":"debug"}));
    tracing::debug!(target: "logging", generation = 8, "new epoch");
    let resumed = call(".recent", json!({"since":first["seq"]}));
    assert!(resumed["seq"].as_u64().unwrap() > first["seq"].as_u64().unwrap());
    assert_eq!(resumed["events"].as_array().unwrap().len(), 2); // set + seam
}
