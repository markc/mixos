// SPDX-License-Identifier: MIT OR Apache-2.0
//! A malformed environment filter must leave live diagnostics usable.
#[test]
fn malformed_environment_uses_a_valid_baseline_and_keeps_live_control() {
    const CHILD: &str = "MIXOS_DEBUG_INVALID_ENV_TEST";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "malformed_environment_uses_a_valid_baseline_and_keeps_live_control", "--nocapture"])
            .env(CHILD, "1")
            .env("RUST_LOG", "[invalid")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("invalid RUST_LOG"));
        return;
    }
    let mut args = vec!["fixture".into()];
    let _guard = logging::debug::init("fixture", "warn", &mut args).unwrap();
    let (rc, initial) = logging::debug::request("fixture", "fixture.debug", "{}").unwrap();
    assert_eq!(rc, 0);
    assert_eq!(initial["baseline"], "warn");
    let (rc, enabled) = logging::debug::request("fixture", "fixture.debug.set", r#"{"level":"debug"}"#).unwrap();
    assert_eq!(rc, 0);
    assert_eq!(enabled["level"], "debug");
    let (rc, restored) = logging::debug::request("fixture", "fixture.debug.set", r#"{"on":false}"#).unwrap();
    assert_eq!(rc, 0);
    assert_eq!(restored["filter"], "warn");
}
