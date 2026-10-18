// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

use config::node::{MaildConfig, NodeConfig, load_from};
use config::{Dir, Error, ErrorKind, from_conf_mix_str, load_conf_mix_path, to_conf_mix_string};

#[test]
fn absent_mail_sections_are_optional_and_not_serialised() {
    let nc: NodeConfig = from_conf_mix_str("node: alpha\nwg_ip: '192.0.2.5'\n").unwrap();
    assert!(nc.maild.is_none());
    assert!(nc.jmap_listen.is_none());
    assert!(nc.jmap_upstream.is_none());
    assert!(nc.smtp_inbound_listen.is_none());
    assert!(nc.smtps_listen.is_none());
    assert_eq!(nc.jmap_listen(), "192.0.2.5:8443");
    assert_eq!(nc.jmap_upstream(), "https://192.0.2.5:8443");
    assert_eq!(nc.smtp_inbound_listen(), "192.0.2.5:25");
    assert_eq!(nc.smtps_listen(), "192.0.2.5:465");
    let encoded = to_conf_mix_string(&nc).unwrap();
    assert!(!encoded.contains("maild"));
    assert!(!encoded.contains("jmap_listen"));
}

#[test]
fn mail_ports_and_address_overrides_are_typed() {
    let nc: NodeConfig = from_conf_mix_str(
        "wg_ip: '192.0.2.5'\nmaild: { jmap_port: 9443, smtp_port: 2525, smtps_port: 2465 }\n",
    )
    .unwrap();
    assert_eq!(nc.jmap_listen(), "192.0.2.5:9443");
    assert_eq!(nc.jmap_upstream(), "https://192.0.2.5:9443");
    assert_eq!(nc.smtp_inbound_listen(), "192.0.2.5:2525");
    assert_eq!(nc.smtps_listen(), "192.0.2.5:2465");
    let nc: NodeConfig = from_conf_mix_str(
        "jmap_listen: '127.0.0.1:8443'\njmap_upstream: 'https://mail.example.org'\nsmtp_inbound_listen: '192.0.2.6:25'\nsmtps_listen: '192.0.2.7:465'\n",
    ).unwrap();
    assert_eq!(nc.jmap_listen(), "127.0.0.1:8443");
    assert_eq!(nc.jmap_upstream(), "https://mail.example.org");
    assert_eq!(nc.smtp_inbound_listen(), "192.0.2.6:25");
    assert_eq!(nc.smtps_listen(), "192.0.2.7:465");
    assert!(nc.extra.is_empty());
    assert!(from_conf_mix_str::<NodeConfig>("maild: { smtp_port: invalid }\n").is_err());
}

#[test]
fn mail_state_defaults_use_native_directory_rules() {
    let m = MaildConfig::default();
    let state = config::path(Dir::Var).join("maild");
    assert_eq!(m.database, state.join("mail.db").to_string_lossy());
    assert_eq!(m.blob_dir, state.join("blobs").to_string_lossy());
    assert_eq!(m.mds_dir, state.join("mds").to_string_lossy());
    assert!(!m.enabled);
    assert!(m.spam_enabled);
}

#[test]
fn typed_mail_round_trip_preserves_future_sections() {
    let nc: NodeConfig = from_conf_mix_str(
        "maild: { future: [a, b], tls: { future: true } }\nwebd: { port: 443 }\n",
    )
    .unwrap();
    let decoded: NodeConfig = from_conf_mix_str(&to_conf_mix_string(&nc).unwrap()).unwrap();
    assert_eq!(nc.extra, decoded.extra);
    let (m, decoded) = (nc.maild.unwrap(), decoded.maild.unwrap());
    assert_eq!(m.extra, decoded.extra);
    assert_eq!(m.tls.extra, decoded.tls.extra);
}

#[test]
fn typed_loader_errors_keep_existing_public_types() {
    let error: Error =
        from_conf_mix_str::<NodeConfig>("maild: { smtp_port: invalid }").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Deserialize);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("node.conf.mix");
    let error: Error = load_conf_mix_path::<NodeConfig>(&file).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Io);
    assert!(error.to_string().contains("node.conf.mix"));
    std::fs::write(
        &file,
        "node: alpha\nmaild: { hostname: 'mail.example.org' }\n",
    )
    .unwrap();
    let nc = load_from(&file).unwrap();
    assert_eq!(nc.maild.unwrap().hostname, "mail.example.org");
    std::fs::write(&file, "maild: { smtp_port: invalid }").unwrap();
    let error: anyhow::Error = load_from(&file).unwrap_err();
    assert!(error.to_string().contains("node.conf.mix"));
    assert!(load_conf_mix_path::<NodeConfig>(&file).is_err());
}

#[test]
fn maild_tls_identities_parse_multi() {
    let src = r#"
            node: "alpha"
            wg_ip: "192.0.2.5"
            maild: {
              hostname: "alpha.example.org",
              tls: {
                identity: [
                  { server_name: "alpha.example.org", cert: "/etc/mixos/maild/tls/lan.crt", key: "/etc/mixos/maild/tls/lan.key", default: true, no_sni_fallback: true },
                  { server_name: "alpha.bus.example.org", cert: "/etc/mixos/maild/tls/wg.crt", key: "/etc/mixos/maild/tls/wg.key" }
                ]
              }
            }
        "#;
    let nc: NodeConfig = from_conf_mix_str(src).expect("parse");
    assert_eq!(nc.maild.as_ref().unwrap().tls.identity.len(), 2);
    assert_eq!(
        nc.maild.as_ref().unwrap().tls.identity[0].server_name,
        "alpha.example.org"
    );
    assert!(nc.maild.as_ref().unwrap().tls.identity[0].default);
    assert!(nc.maild.as_ref().unwrap().tls.identity[0].no_sni_fallback);
    assert_eq!(
        nc.maild.as_ref().unwrap().tls.identity[1].server_name,
        "alpha.bus.example.org"
    );
    assert!(!nc.maild.as_ref().unwrap().tls.identity[1].default);
    assert!(!nc.maild.as_ref().unwrap().tls.identity[1].no_sni_fallback);
    assert!(!nc.maild.as_ref().unwrap().tls.strict_sni);
}

#[test]
fn maild_tls_strict_sni_toggle() {
    let src = "maild: { tls: { strict_sni: true } }\n";
    let nc: NodeConfig = from_conf_mix_str(src).expect("parse");
    assert!(nc.maild.as_ref().unwrap().tls.strict_sni);
    assert!(nc.maild.as_ref().unwrap().tls.identity.is_empty());
}

#[test]
fn maild_tls_legacy_only_leaves_identity_empty() {
    let src = "maild: { tls_cert: \"/etc/mixos/maild/tls/legacy.crt\", \
                   tls_key: \"/etc/mixos/maild/tls/legacy.key\" }\n";
    let nc: NodeConfig = from_conf_mix_str(src).expect("parse");
    assert!(nc.maild.as_ref().unwrap().tls.identity.is_empty());
    assert_eq!(
        nc.maild.as_ref().unwrap().tls_cert.as_deref(),
        Some("/etc/mixos/maild/tls/legacy.crt"),
        "legacy pair must still parse unchanged"
    );
}
#[test]
fn maild_tls_identity_row_missing_required_field_fails_parse() {
    // An operator writing a tls identity row but forgetting `cert`
    // (or `key`, or `server_name`) must hit a hard parse error at
    // startup, not silently inherit empty strings that would later
    // shadow the legacy `tls_cert` / `tls_key` pair with an
    // unservable identity. `TlsIdentityConfig` has no
    // `#[serde(default)]`, so the three string fields are required.
    let cases = [
        // missing cert
        "maild: { tls_cert: \"/etc/legacy.crt\", tls_key: \"/etc/legacy.key\", \
             tls: { identity: [ { server_name: \"host.example\", key: \"/etc/host.key\" } ] } }\n",
        // missing key
        "maild: { tls: { identity: [ { server_name: \"host.example\", cert: \"/etc/host.crt\" } ] } }\n",
        // missing server_name
        "maild: { tls: { identity: [ { cert: \"/etc/host.crt\", key: \"/etc/host.key\" } ] } }\n",
    ];
    for (i, src) in cases.iter().enumerate() {
        let res: Result<NodeConfig, _> = from_conf_mix_str(src);
        assert!(
            res.is_err(),
            "case {i}: malformed identity row must fail parse, got Ok"
        );
    }
}
