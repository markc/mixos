// SPDX-License-Identifier: MIT OR Apache-2.0
//! The real log-to-tracing bridge must follow live controls in both directions.
use serde_json::{Value, json};

#[test]
fn dependency_records_follow_live_levels() {
    if std::env::var_os("MIXOS_DEBUG_FACADE_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "dependency_records_follow_live_levels",
                "--nocapture",
            ])
            .env("MIXOS_DEBUG_FACADE_CHILD", "1")
            .env_remove("RUST_LOG")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    let _guard = logging::debug::init("fixture", "warn", &mut vec!["fixture".into()]).unwrap();
    let set = |level: &str| {
        assert_eq!(
            logging::debug::request(
                "fixture",
                "fixture.debug.set",
                &json!({"level":level}).to_string()
            )
            .unwrap()
            .0,
            0
        );
    };
    let recent = || -> Value {
        logging::debug::request("fixture", "fixture.debug.recent", "{}")
            .unwrap()
            .1
    };
    set("info");
    log::info!(target: "fake_dependency", "dependency info admitted");
    assert!(
        recent()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["message"] == "dependency info admitted")
    );
    set("debug");
    // A fake dependency emitting into an admitted application target. Ordinary
    // external targets remain capped at info by the live-debug policy.
    log::debug!(target: "ced::fake_dependency", "dependency debug admitted");
    assert!(
        recent()["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["message"] == "dependency debug admitted")
    );
    set("warn");
    let evaluated = std::cell::Cell::new(false);
    log::debug!(target: "ced::fake_dependency", "{}", {
        evaluated.set(true);
        "dependency debug rejected"
    });
    assert!(
        !evaluated.get(),
        "the facade must cut the record before evaluating it"
    );
    assert!(recent()["events"].as_array().unwrap().is_empty());
}
