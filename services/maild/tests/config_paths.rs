// SPDX-License-Identifier: MIT OR Apache-2.0
use maild::config::Config;

#[test]
fn all_config_paths_resolve_through_both_loaders() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.conf.mix");
    let text = r#"{
        database: "mixos:var/maild/mail.db",
        blob_dir: "mixos:var/maild/blobs", mds_dir: "mixos:var/maild/mds",
        dkim_private_key: "mixos:etc/maild/dkim.pem",
        tls_cert: "mixos:etc/maild/cert.pem", tls_key: "mixos:etc/maild/key.pem",
        tls_key_root: "mixos:var/maild/tls",
        inbound_filter: "mixos:etc/maild/filter.mix",
        spam_db_dir: "mixos:var/maild/spam", spam_baseline_db: "mixos:share/maild/spam.db",
        rules_pack_path: "mixos:etc/maild/rules.conf.mix", rule_stats_dir: "mixos:var/maild/rules",
        dkim: {key_root: "mixos:var/maild/dkim", domain: [{domain: "example.com", selector: "default",
            algorithm: "rsa-sha256", key_path: "mixos:etc/maild/domain.pem"}]},
        tls: {identity: [{server_name: "example.com", cert: "mixos:etc/maild/sni.pem", key: "mixos:etc/maild/sni.key"}]}
    }"#;
    std::fs::write(&path, text).unwrap();
    for cfg in [Config::load(path.to_str().unwrap()).unwrap(), Config::try_load_optional(&path).unwrap().unwrap()] {
        let pairs = [
            (cfg.database_path, "mixos:var/maild/mail.db"),
            (cfg.blob_dir, "mixos:var/maild/blobs"), (cfg.mds_dir, "mixos:var/maild/mds"),
            (cfg.dkim_private_key.unwrap(), "mixos:etc/maild/dkim.pem"),
            (cfg.tls_cert.unwrap(), "mixos:etc/maild/cert.pem"), (cfg.tls_key.unwrap(), "mixos:etc/maild/key.pem"),
            (cfg.tls_key_root.unwrap().to_str().unwrap().into(), "mixos:var/maild/tls"),
            (cfg.inbound_filter.unwrap(), "mixos:etc/maild/filter.mix"),
            (cfg.spam_db_dir.unwrap(), "mixos:var/maild/spam"),
            (cfg.spam_baseline_db.unwrap(), "mixos:share/maild/spam.db"),
            (cfg.rules_pack_path.unwrap(), "mixos:etc/maild/rules.conf.mix"),
            (cfg.rule_stats_dir.unwrap(), "mixos:var/maild/rules"),
            (cfg.dkim.key_root.unwrap().to_str().unwrap().into(), "mixos:var/maild/dkim"),
            (cfg.dkim.domains[0].key_path.to_str().unwrap().into(), "mixos:etc/maild/domain.pem"),
            (cfg.tls.identity[0].cert.clone(), "mixos:etc/maild/sni.pem"),
            (cfg.tls.identity[0].key.clone(), "mixos:etc/maild/sni.key"),
        ];
        for (actual, logical) in pairs { assert_eq!(std::path::Path::new(&actual), paths::resolve(logical).unwrap()); }
    }
}

#[test]
fn invalid_logical_paths_fail_and_user_paths_keep_their_meaning() {
    for invalid in ["mixos:var/../escape", "mixos:unknown/file"] {
        let cfg = Config { blob_dir: invalid.into(), ..Default::default() };
        assert!(cfg.resolve_paths().is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.conf.mix");
        std::fs::write(&path, format!("{{blob_dir: \"{invalid}\"}}")).unwrap();
        assert!(Config::load(path.to_str().unwrap()).is_err());
        assert!(Config::try_load_optional(&path).is_err());
    }
    let cfg = Config { database_path: "relative/mail.db".into(), blob_dir: "/home/user/blobs".into(), ..Default::default() }.resolve_paths().unwrap();
    assert_eq!(cfg.database_path, "relative/mail.db");
    assert_eq!(cfg.blob_dir, "/home/user/blobs");
}
