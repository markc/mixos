// SPDX-License-Identifier: MIT OR Apache-2.0
//! A malformed environment filter must leave live diagnostics usable.
#[test]
fn informational_requests_install_nothing() {
    const CHILD: &str = "MIXOS_DEBUG_INFO_TEST";
    if let Ok(flag) = std::env::var(CHILD) {
        let mut args = vec!["fixture".into(), "--debug=trace".into(), flag];
        let guard = logging::debug::init("fixture", "warn", &mut args).unwrap();
        assert!(guard.reload_handle().is_none());
        assert_eq!(logging::debug::request("fixture", "fixture.debug", "{}").unwrap().0, 10);
        // Neither the global tracing slot nor LogTracer was installed.
        tracing::subscriber::set_global_default(tracing_subscriber::registry()).unwrap();
        tracing_log::LogTracer::init().unwrap();
        return;
    }
    for flag in ["--help", "-h", "--version", "-V"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "informational_requests_install_nothing", "--nocapture"])
            .env(CHILD, flag).env("RUST_LOG", "[invalid")
            .output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

#[test]
fn malformed_environment_uses_a_valid_baseline_and_keeps_live_control() {
    const CHILD: &str = "MIXOS_DEBUG_INVALID_ENV_TEST";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "malformed_environment_uses_a_valid_baseline_and_keeps_live_control",
                "--nocapture",
            ])
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
    let (rc, enabled) =
        logging::debug::request("fixture", "fixture.debug.set", r#"{"level":"debug"}"#).unwrap();
    assert_eq!(rc, 0);
    assert_eq!(enabled["level"], "debug");
    let (rc, restored) =
        logging::debug::request("fixture", "fixture.debug.set", r#"{"on":false}"#).unwrap();
    assert_eq!(rc, 0);
    assert_eq!(restored["filter"], "warn");
}
