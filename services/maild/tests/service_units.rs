// SPDX-License-Identifier: MIT OR Apache-2.0
#[test]
fn config_binds_keep_capability_until_host_activation_is_configured() {
    for unit in [include_str!("../units/maild.service"), include_str!("../units/maild@.service")] {
        assert!(unit.lines().any(|s| s == "CapabilityBoundingSet=CAP_NET_BIND_SERVICE"));
        assert!(unit.lines().any(|s| s == "AmbientCapabilities=CAP_NET_BIND_SERVICE"));
    }
    for (protocol, unit) in [
        ("smtp", include_str!("../units/maild-smtp@.socket")),
        ("submissions", include_str!("../units/maild-submissions@.socket")),
        ("imaps", include_str!("../units/maild-imaps@.socket")),
        ("jmap", include_str!("../units/maild-jmap@.socket")),
    ] {
        assert!(unit.lines().any(|s| s == "Service=maild@%i.service"));
        assert!(unit.lines().any(|s| s == format!("FileDescriptorName={protocol}")));
        assert!(!unit.lines().any(|s| s.starts_with("ListenStream=")), "host must choose its configured addresses");
    }
}
