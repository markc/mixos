// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Exercise the public paths retained when model's nested wrappers are dissolved.

#[cfg(all(
    feature = "error",
    feature = "warn",
    feature = "info",
    feature = "trace"
))]
#[test]
fn exported_macros_keep_the_callers_identity_and_runtime_gate() {
    use model::debug::instance::record;
    let (tx, rx) = crossbeam_channel::bounded(8);
    assert!(record::install_sender(tx));
    record::set_enabled_mask(record::parse_levels("error,warn,info,trace"));
    model::error!("error {}", 17);
    model::warn!("warning");
    model::info!("information");
    model::trace!("trace");
    for (level, text) in [
        (record::Level::Error, "error 17"),
        (record::Level::Warn, "warning"),
        (record::Level::Info, "information"),
        (record::Level::Trace, "trace"),
    ] {
        let emitted = rx.try_recv().unwrap();
        assert_eq!(emitted.level, level);
        assert_eq!(emitted.message, text);
        assert_eq!(emitted.crate_name, module_path!());
        assert!(
            emitted
                .function
                .ends_with("exported_macros_keep_the_callers_identity_and_runtime_gate")
        );
    }
    record::set_enabled_mask(0);
    model::info!("filtered");
    assert!(rx.try_recv().is_err());
}

#[cfg(not(any(
    feature = "error",
    feature = "warn",
    feature = "info",
    feature = "trace"
)))]
#[test]
fn disabled_macros_do_not_evaluate_their_arguments() {
    model::error!("{}", panic!("disabled error"));
    model::warn!("{}", panic!("disabled warning"));
    model::info!("{}", panic!("disabled information"));
    model::trace!("{}", panic!("disabled trace"));
}

#[test]
fn the_log_drain_acknowledges_a_record_through_the_public_entry_point() {
    use model::debug::instance::record::{Level, Record};
    let (tx, rx) = crossbeam_channel::bounded(1);
    let (ack, received) = crossbeam_channel::bounded(1);
    model::log::process::instance::start(rx);
    let mut record = Record::with(Level::Info, "example", "example::drain", "delivered".into());
    record.ack = Some(ack);
    tx.send(record).unwrap();
    received
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
}

#[test]
fn tearing_configuration_keeps_its_serialised_shape() {
    use model::environment::tearing::{config::Config, rate::Rate, select::Scene};
    let config = Config::default();
    assert_eq!(
        serde_json::to_value(config).unwrap(),
        serde_json::json!({
            "tearing": {"selector": "TargetFocused", "mode": "Always", "rate": "Uncapped", "exclusivity": "ExclusiveFocused"},
            "pacing": {"selector": "Never", "mode": "Fixed", "rate": "Uncapped", "exclusivity": "None"},
            "tag": {"steam": true}, "floor": {"Multiplier": 1.0}
        })
    );
    assert!(!config.tearing.selector.matches(Scene::default()));
    assert!(config.tearing.selector.matches(Scene {
        target_focused: true,
        ..Scene::default()
    }));
    assert_eq!(
        Rate::Multiplier(2.0).min_interval(std::time::Duration::from_millis(16)),
        Some(std::time::Duration::from_millis(8))
    );
}

#[test]
fn registry_facade_still_reads_the_published_device_format() {
    use model::stats::registry::base;
    base::set_device_format("module-test", "Argb8888", 0, "linear", 1);
    let snapshot = base::snapshot();
    let device = snapshot
        .device_formats
        .iter()
        .find(|device| device.kind == "module-test")
        .unwrap();
    assert_eq!(device.fourcc, "Argb8888");
    assert_eq!(device.modifier, "0x0000000000000000");
    assert!(!device.multiplane);
}
