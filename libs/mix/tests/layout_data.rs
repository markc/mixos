// SPDX-License-Identifier: MIT OR Apache-2.0
//! Same fixtures as paths' reader, through the real load_data builtin.
#[tokio::test(flavor = "current_thread")]
async fn layout_reader_and_load_data_share_a_grammar() {
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../paths/tests/layout");
    for entry in std::fs::read_dir(fixtures).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap();
        let source = std::fs::read_to_string(&path).unwrap();
        let quoted = format!("{:?}", path.to_string_lossy()).replace("${", "\\${");
        let loaded = mix::run(&format!("load_data({quoted})")).await;
        let resolved = paths::parse_layout(&source);
        assert_eq!(
            loaded.is_ok(),
            !name.starts_with("syntax_"),
            "load_data: {name}"
        );
        assert_eq!(
            resolved.is_ok(),
            name.starts_with("ok_"),
            "paths reader: {name}"
        );
        if name.starts_with("ok_") {
            let loaded = loaded.unwrap();
            let mix::value::Value::Map(map) = &loaded else {
                panic!("{name}: load_data did not return a map")
            };
            let roots = resolved.unwrap();
            assert_eq!(map.len(), roots.len(), "{name}");
            for (root, value) in roots {
                let Some(mix::value::Value::String(text)) = map.get(root.name()) else {
                    panic!("{name}: missing string root")
                };
                assert_eq!(std::path::Path::new(text), value, "{name}");
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mix_front_door_selects_the_explicit_tier_and_rejects_old_root_names() {
    const CHILD: &str = "MIXOS_LAYOUT_FRONT_DOOR_TEST";
    const TEST: &str = "mix_front_door_selects_the_explicit_tier_and_rejects_old_root_names";
    if std::env::var_os(CHILD).is_none() {
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let root =
            Scratch(std::env::temp_dir().join(format!("mixos-front-door-{}", std::process::id())));
        std::fs::create_dir(&root.0).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, "1")
            .env("MIXOS_PREFIX", &root.0)
            .env("HOME", root.0.join("home"))
            .env("XDG_CONFIG_HOME", root.0.join("config"))
            .env("XDG_DATA_HOME", root.0.join("data"))
            .env("XDG_RUNTIME_DIR", root.0.join("runtime"))
            .env("XDG_CACHE_HOME", root.0.join("cache"))
            .env_remove("MIXOS")
            .env_remove("MIXOS_ETC")
            .env_remove("MIXOS_VAR")
            .env_remove("MIXOS_RUN")
            .env_remove("MIXOS_SHARE")
            .env_remove("MIXOS_CACHE")
            .output()
            .unwrap();
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"),
            "front-door child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    // Self-location is never used: the child received every path input before
    // the process-global resolver was first called.
    let prefix =
        std::env::var_os("MIXOS_PREFIX").expect("front-door test needs an explicit prefix");
    for (tier, uid) in [("system", 0), ("user", 1000)] {
        let inputs = paths::Environment {
            uid,
            ..paths::Environment::current()
        };
        let expected = paths::Layout::from_environment(&inputs).unwrap();
        assert_eq!(
            expected.root(paths::Root::Prefix),
            std::path::PathBuf::from(&prefix)
        );
        for root in paths::Root::ALL {
            let source = format!(
                "mixos_path(\"{}\", \"sentinel\", {{tier: \"{tier}\"}})",
                root.name()
            );
            let value = mix::run(&source).await.unwrap();
            let mix::value::Value::String(value) = &value else {
                panic!("path builtin did not return a string")
            };
            assert_eq!(
                std::path::Path::new(&value),
                expected.path(root, "sentinel").unwrap(),
                "{tier} {root:?}"
            );
        }
    }
    for root in ["config", "state", "runtime", "bin", "lib"] {
        assert!(
            mix::run(&format!("mixos_path(\"{root}\")")).await.is_err(),
            "old/unknown root accepted: {root}"
        );
    }
    for options in [
        "{tier: 'invalid'}",
        "{tier: 'system', unknown: true}",
        "'system'",
    ] {
        assert!(
            mix::run(&format!("mixos_path('etc', '', {options})"))
                .await
                .is_err()
        );
    }
}
