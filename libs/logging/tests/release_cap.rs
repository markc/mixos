// SPDX-License-Identifier: MIT OR Apache-2.0
#[cfg(all(feature = "release-max-level-warn", not(debug_assertions)))]
#[test]
fn tiny_release_compiles_verbose_sites_out() {
    assert_eq!(tracing::level_filters::STATIC_MAX_LEVEL, tracing::level_filters::LevelFilter::WARN);
    let _guard = logging::debug::init("fixture", "warn", &mut vec!["fixture".into()]).unwrap();
    logging::debug::request("fixture", "fixture.debug.set", r#"{"level":"trace"}"#).unwrap();
    assert!(!tracing::enabled!(target: "logging", tracing::Level::DEBUG));
    assert!(!tracing::enabled!(target: "logging", tracing::Level::TRACE));
    let evaluated = std::cell::Cell::new(false);
    tracing::debug!(value = { evaluated.set(true); 1 }, "compiled out");
    assert!(!evaluated.get());
}
