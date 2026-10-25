// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn public_live_environments_match_main_for_both_uids_and_prefixes() {
    // Expected Etc/Var/Run values were transcribed by applying the match arms
    // of 32650ae5:libs/config/src/dir.rs by hand, not by a second resolver.
    // Cache is additive: the same uid rule with XDG_CACHE_HOME/home/.cache.
    // Share stays pinned by MIXOS_SHARE in desk fixtures; otherwise it moves.
    struct Fixture {
        name: &'static str,
        env: &'static [(&'static str, &'static str)],
        system: [&'static str; 4],
        user: [&'static str; 4],
    }
    let fixtures = [
        Fixture {
            name: "desk-isolated",
            env: &[
                ("HOME", "/home/user"),
                ("MIXOS_ETC", "/var/lib/mixos/desk/etc"),
                ("MIXOS_VAR", "/var/lib/mixos/desk/var"),
                ("MIXOS_RUN", "/run/mixos/session/3"),
                ("MIXOS_SHARE", "/opt/mixos/share"),
                ("XDG_RUNTIME_DIR", "/run/mixos/session/3/desktop"),
            ],
            system: [
                "/var/lib/mixos/desk/etc",
                "/var/lib/mixos/desk/var",
                "/run/mixos/session/3",
                "/var/cache/mixos",
            ],
            user: [
                "/var/lib/mixos/desk/etc",
                "/var/lib/mixos/desk/var",
                "/run/mixos/session/3",
                "/home/user/.cache/mixos",
            ],
        },
        Fixture {
            name: "desk-shared",
            env: &[
                ("HOME", "/home/user"),
                ("MIXOS_ETC", "/var/lib/mixos/vt/etc"),
                ("MIXOS_VAR", "/var/lib/mixos/vt/var"),
                ("MIXOS_RUN", "/run/mixos/session/4"),
                ("MIXOS_SHARE", "/opt/mixos/share"),
                ("XDG_RUNTIME_DIR", "/run/mixos/session/4/desktop"),
                ("XDG_CONFIG_HOME", "/var/lib/mixos/vt/home/config"),
                ("XDG_DATA_HOME", "/var/lib/mixos/vt/home/data"),
            ],
            system: [
                "/var/lib/mixos/vt/etc",
                "/var/lib/mixos/vt/var",
                "/run/mixos/session/4",
                "/var/cache/mixos",
            ],
            user: [
                "/var/lib/mixos/vt/etc",
                "/var/lib/mixos/vt/var",
                "/run/mixos/session/4",
                "/home/user/.cache/mixos",
            ],
        },
        Fixture {
            name: "system-unit",
            env: &[("MIXOS_VAR", "/var/lib/mixos")],
            system: [
                "/etc/mixos",
                "/var/lib/mixos",
                "/run/mixos",
                "/var/cache/mixos",
            ],
            user: [
                "/root/.config/mixos",
                "/var/lib/mixos",
                "/tmp/mixos-run",
                "/root/.cache/mixos",
            ],
        },
        Fixture {
            name: "bare-login",
            env: &[
                ("HOME", "/home/user"),
                ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ],
            system: [
                "/etc/mixos",
                "/var/lib/mixos",
                "/run/mixos",
                "/var/cache/mixos",
            ],
            user: [
                "/home/user/.config/mixos",
                "/home/user/.local/share/mixos",
                "/run/user/1000/mixos",
                "/home/user/.cache/mixos",
            ],
        },
        Fixture {
            name: "bare-root",
            env: &[("HOME", "/root")],
            system: [
                "/etc/mixos",
                "/var/lib/mixos",
                "/run/mixos",
                "/var/cache/mixos",
            ],
            user: [
                "/root/.config/mixos",
                "/root/.local/share/mixos",
                "/tmp/mixos-run",
                "/root/.cache/mixos",
            ],
        },
    ];
    for fixture in fixtures {
        for uid in [0, 1000] {
            for prefix in ["/opt/mixos", "/srv/moved tree"] {
                let get = |key: &str| -> Option<PathBuf> {
                    fixture
                        .env
                        .iter()
                        .find(|(name, _)| *name == key)
                        .map(|(_, value)| PathBuf::from(*value))
                };
                let env = Environment::from_lookup(get, uid, Some(Path::new(prefix).join("bin/x")));
                let layout = Layout::from_source(&env, "").unwrap();
                let expected = if uid == 0 {
                    fixture.system
                } else {
                    fixture.user
                };
                for (root, value) in [Root::Etc, Root::Var, Root::Run, Root::Cache]
                    .into_iter()
                    .zip(expected)
                {
                    assert_eq!(
                        layout.root(root),
                        Path::new(value),
                        "{} uid={uid} prefix={prefix} {root:?}",
                        fixture.name
                    );
                }
                assert_eq!(
                    layout.root(Root::Prefix),
                    Path::new(prefix),
                    "{}",
                    fixture.name
                );
                let share = get("MIXOS_SHARE").unwrap_or_else(|| Path::new(prefix).join("share"));
                assert_eq!(
                    layout.root(Root::Share),
                    share,
                    "{} uid={uid}",
                    fixture.name
                );
            }
        }
    }
}

#[test]
fn precedence_layout_tiers_and_non_utf8_inputs() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let env = Environment {
        root: Some("/whole tree".into()),
        etc: Some("/override/etc".into()),
        uid: 1000,
        home: Some("/home/user".into()),
        exe: Some("/opt/mixos/bin/x".into()),
        ..Environment::default()
    };
    let source = "etc: '/file/etc'\nvar: '/file/var'\nshare: '/file/share'\ncache: '/file/cache'";
    let layout = Layout::from_source(&env, source).unwrap();
    assert_eq!(layout.root(Root::Etc), Path::new("/override/etc"));
    assert_eq!(layout.root(Root::Var), Path::new("/whole tree/var"));
    assert_eq!(layout.root(Root::Run), Path::new("/whole tree/run"));
    assert_eq!(layout.root(Root::Cache), Path::new("/whole tree/cache"));
    assert_eq!(layout.root(Root::Share), Path::new("/file/share"));
    assert!(parse_layout("prefix: '/wrong'").is_err());
    let user = Layout::defaults("user", Some(Path::new("/opt/mixos")), &env).unwrap();
    let system = Layout::defaults("system", Some(Path::new("/home/user/install")), &env).unwrap();
    assert_eq!(
        user.root(Root::Var),
        Path::new("/home/user/.local/share/mixos")
    );
    assert_eq!(user.root(Root::Run), Path::new("/tmp/mixos-run"));
    assert_eq!(system.root(Root::Etc), Path::new("/etc/mixos"));
    let non_utf8 = PathBuf::from(OsString::from_vec(b"/home/non-utf8-\xff".to_vec()));
    let env = Environment::from_lookup(
        |key: &str| {
            if key == "HOME" {
                Some(non_utf8.clone())
            } else {
                None
            }
        },
        1000,
        Some("/srv/tree/bin/x".into()),
    );
    assert_eq!(
        Layout::resolve(&env).get(Dir::Var),
        non_utf8.join(".local/share/mixos")
    );
}

#[test]
fn relative_xdg_cache_is_ignored_and_relative_legacy_overrides_are_retained() {
    let env = Environment {
        uid: 1000,
        home: Some("/home/user".into()),
        xdg_cache_home: Some("relative".into()),
        etc: Some("relative-etc".into()),
        var: Some("relative-var".into()),
        run: Some("relative-run".into()),
        ..Environment::default()
    };
    let l = Layout::resolve(&env);
    assert_eq!(l.root(Root::Cache), Path::new("/home/user/.cache/mixos"));
    assert_eq!(l.get(Dir::Etc), Path::new("relative-etc"));
    assert_eq!(l.get(Dir::Var), Path::new("relative-var"));
    assert_eq!(l.get(Dir::Run), Path::new("relative-run"));
}

#[test]
fn explicit_prefix_and_all_file_roots_have_the_documented_precedence() {
    let env = Environment {
        prefix: Some("/chosen prefix".into()),
        exe: Some("/srv/moved tree/bin/x".into()),
        uid: 1000,
        home: Some("/home/user".into()),
        ..Environment::default()
    };
    let source = "etc: '/file/etc'\nvar: '/file/var'\nrun: '/file/run'\nshare: '/file/share'\ncache: '/file/cache'";
    let layout = Layout::from_source(&env, source).unwrap();
    assert_eq!(layout.root(Root::Prefix), Path::new("/chosen prefix"));
    for root in [Root::Etc, Root::Var, Root::Run, Root::Share, Root::Cache] {
        assert_eq!(layout.root(root), Path::new("/file").join(root.name()));
    }
    let overrides = Environment {
        etc: Some("/override/etc".into()),
        var: Some("/override/var".into()),
        run: Some("/override/run".into()),
        share: Some("/override/share".into()),
        cache: Some("/override/cache".into()),
        ..env
    };
    let layout = Layout::from_source(&overrides, source).unwrap();
    for root in [Root::Etc, Root::Var, Root::Run, Root::Share, Root::Cache] {
        assert_eq!(layout.root(root), Path::new("/override").join(root.name()));
    }
    let empty = Environment::from_lookup(
        |_: &str| Some(PathBuf::new()),
        1000,
        Some("/srv/moved tree/bin/x".into()),
    );
    assert_eq!(empty.prefix, None);
    assert_eq!(empty.root, None);
    assert_eq!(
        Layout::from_source(&empty, "").unwrap().root(Root::Prefix),
        Path::new("/srv/moved tree")
    );
}

#[test]
fn logical_paths_are_bounded_and_layout_fixture_contract() {
    let env = Environment {
        exe: Some("/srv/moved tree/bin/x".into()),
        ..Environment::default()
    };
    let l = Layout::resolve(&env);
    assert_eq!(
        l.resolve_path("mixos:share/a/../b").unwrap(),
        Path::new("/srv/moved tree/share/b")
    );
    for path in [
        "mixos:prefix/../a",
        "mixos:etc/a/../../b",
        "mixos:config/a",
        "mixos:prefix//etc",
    ] {
        assert!(l.resolve_path(path).is_err(), "{path}");
    }
    for entry in
        std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/layout")).unwrap()
    {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap();
        let source = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            parse_layout(&source).is_ok(),
            name.starts_with("ok_"),
            "{name}"
        );
    }
    assert!(parse_layout(&" ".repeat(65537)).is_err());
}

#[test]
fn real_layout_and_symlink_follow_a_move() {
    let temp = std::env::temp_dir().join(format!("mixos-paths-{}", std::process::id()));
    let first = temp.join("first prefix");
    let moved = temp.join("moved prefix");
    std::fs::create_dir_all(first.join("bin")).unwrap();
    std::fs::write(first.join("bin/x"), "fixture").unwrap();
    std::fs::write(
        first.join("layout.mx"),
        "etc: '/file/etc'\nvar: '/file/var'",
    )
    .unwrap();
    let mut env = Environment {
        uid: 1000,
        home: Some("/home/user".into()),
        exe: Some(first.join("bin/x")),
        ..Environment::default()
    };
    assert_eq!(
        Layout::from_environment(&env).unwrap().get(Dir::Var),
        Path::new("/file/var")
    );
    std::fs::rename(&first, &moved).unwrap();
    let entry = temp.join("entry");
    std::os::unix::fs::symlink(moved.join("bin/x"), &entry).unwrap();
    env.exe = Some(entry);
    let after = Layout::from_environment(&env).unwrap();
    assert_eq!(after.root(Root::Prefix), moved);
    assert_eq!(after.root(Root::Share), moved.join("share"));
    assert_eq!(after.get(Dir::Etc), Path::new("/file/etc"));
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn a_cargo_build_tree_is_not_an_install() {
    let temp = std::env::temp_dir().join(format!("mixos-paths-cargo-{}", std::process::id()));
    let profile = temp.join("target/debug");
    std::fs::create_dir_all(profile.join(".fingerprint")).unwrap();
    std::fs::create_dir_all(profile.join("deps")).unwrap();
    for exe in [profile.join("term"), profile.join("deps/term-0123")] {
        assert_eq!(
            prefix_from_exe(&exe).unwrap(),
            Path::new(DEFAULT_PREFIX),
            "{}",
            exe.display()
        );
    }
    // MIXOS_PREFIX still wins over the build-tree default.
    let env = Environment {
        uid: 1000,
        home: Some("/home/user".into()),
        exe: Some(profile.join("deps/term-0123")),
        prefix: Some("/srv/moved tree".into()),
        ..Environment::default()
    };
    assert_eq!(
        Layout::from_environment(&env).unwrap().root(Root::Prefix),
        Path::new("/srv/moved tree")
    );
    std::fs::remove_dir_all(temp).unwrap();
}
