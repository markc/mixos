// SPDX-License-Identifier: MIT OR Apache-2.0
use macros::{Catalogue, Entry, OUTPUT_LIMIT, Roots, Runner, Status, Tier, valid_id};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn roots(dir: &Path) -> Roots {
    Roots {
        builtin: dir.join("builtin"),
        user: dir.join("user"),
        home: dir.into(),
        mix: PathBuf::from("/opt/mixos/bin/mix"),
    }
}

#[test]
fn roots_are_pure_absolute_and_injectable() {
    let home = PathBuf::from("/home/user");
    let resolved = Roots::resolve_with(|key| match key {
        "HOME" => Some(home.clone()),
        _ => Some("relative".into()),
    })
    .unwrap();
    assert_eq!(resolved.builtin, Path::new("/opt/mixos/share/macros"));
    assert_eq!(resolved.user, home.join(".config/mixos/macros"));
    assert_eq!(resolved.mix, Path::new("/opt/mixos/bin/mix"));
    let resolved = Roots::resolve_with(|key| {
        Some(PathBuf::from(match key {
            "HOME" => "/home/user",
            "MIXOS_SHARE" => "/share",
            "XDG_CONFIG_HOME" => "/config",
            "MIXOS_MIX" => "/bin/custom-mix",
            _ => unreachable!(),
        }))
    })
    .unwrap();
    assert_eq!(resolved.user, Path::new("/config/mixos/macros"));
    assert_eq!(resolved.builtin, Path::new("/share/macros"));
    assert_eq!(resolved.mix, Path::new("/bin/custom-mix"));
    assert!(Roots::resolve_with(|_| None).is_none());
    assert!(Roots::resolve_with(|_| Some("relative".into())).is_none());
}

#[test]
fn ids_refuse_paths_dotfiles_unicode_and_uppercase() {
    for id in ["a", "a1-b_c", "123", "-"] {
        assert!(valid_id(id), "{id}");
    }
    for id in [
        "", ".hidden", "../other", "/path", "Upper", "a b", "a.mix", "ä", "a\\b",
    ] {
        assert!(!valid_id(id), "{id}");
    }
}

#[test]
fn catalogue_precedence_titles_and_diagnostics_are_deterministic() {
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    assert!(Catalogue::scan(&roots, "ced").unwrap().entries.is_empty());
    for tier in Tier::ALL {
        let folder = roots.folder("ced", tier).unwrap();
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("shared.mix"),
            format!(
                "-- title: {}\nraise(\"DO_NOT_RUN\", \"scan\")",
                tier.as_str()
            ),
        )
        .unwrap();
        fs::write(
            folder.join(format!("{}.mix", tier.as_str())),
            "-- header\n-- title: A title\n",
        )
        .unwrap();
    }
    fs::write(roots.user.join("z.mix"), "print(1)").unwrap();
    fs::write(roots.user.join(".hidden.mix"), "").unwrap();
    fs::write(roots.user.join("Bad.mix"), "").unwrap();
    fs::write(roots.user.join("document.mx"), "").unwrap();
    fs::create_dir(roots.user.join("directory.mix")).unwrap();
    let found = Catalogue::scan(&roots, "ced").unwrap();
    let ids: Vec<_> = found.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "builtin-app",
            "builtin-global",
            "shared",
            "user-app",
            "user-global",
            "z"
        ]
    );
    let shared = &found.entries[2];
    assert_eq!(shared.tier, Tier::UserApp);
    assert_eq!(shared.title, "user-app");
    assert_eq!(
        shared.shadows,
        [Tier::BuiltinGlobal, Tier::BuiltinApp, Tier::UserGlobal]
    );
    assert_eq!(found.entries[5].title, "z");
    assert_eq!(found.diagnostics.len(), 3);
    // Removing each winner exposes exactly the next tier.
    for expected in Tier::ALL.into_iter().rev() {
        let found = Catalogue::scan(&roots, "ced").unwrap();
        let entry = found.entries.iter().find(|e| e.id == "shared").unwrap();
        assert_eq!(entry.tier, expected);
        fs::remove_file(&entry.path).unwrap();
    }
    assert!(Catalogue::scan(&roots, "../escape").is_err());
}

#[test]
fn create_and_copy_never_overwrite_the_destination_tier() {
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    let path = roots
        .create("ced", Tier::UserApp, "new", "New macro")
        .unwrap();
    let before = fs::read(&path).unwrap();
    assert!(
        roots
            .create("ced", Tier::UserApp, "new", "Overwrite")
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), before);
    assert!(
        roots
            .create("ced", Tier::BuiltinGlobal, "bad", "Bad")
            .is_err()
    );
    assert!(roots.create("ced", Tier::UserApp, "../bad", "Bad").is_err());
    assert!(
        roots
            .create("ced", Tier::UserApp, "bad", "Injected\nprint(1)")
            .is_err()
    );
    for tier in [Tier::BuiltinGlobal, Tier::BuiltinApp] {
        let folder = roots.folder("ced", tier).unwrap();
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("copy.mix");
        fs::write(&path, "-- title: Original\nprint(1)\n").unwrap();
        let entry = Entry {
            id: "copy".into(),
            title: "Original".into(),
            path,
            tier,
            shadows: vec![],
        };
        let copy = roots.copy_to_user("ced", &entry).unwrap();
        assert_eq!(
            copy,
            roots
                .folder(
                    "ced",
                    if tier.is_app() {
                        Tier::UserApp
                    } else {
                        Tier::UserGlobal
                    }
                )
                .unwrap()
                .join("copy.mix")
        );
        assert_eq!(fs::read(&copy).unwrap(), fs::read(&entry.path).unwrap());
        assert!(roots.copy_to_user("ced", &entry).is_err());
    }
}

#[cfg(unix)]
#[test]
fn symlinks_may_stay_inside_but_must_never_escape_the_root() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    fs::create_dir(&roots.user).unwrap();
    fs::write(roots.user.join("original.mix"), "print(1)").unwrap();
    fs::write(temp.path().join("outside.mix"), "print(2)").unwrap();
    symlink(
        roots.user.join("original.mix"),
        roots.user.join("inside.mix"),
    )
    .unwrap();
    symlink(
        temp.path().join("outside.mix"),
        roots.user.join("escape.mix"),
    )
    .unwrap();
    let found = Catalogue::scan(&roots, "ced").unwrap();
    assert_eq!(found.entries.len(), 2);
    assert_eq!(found.diagnostics.len(), 1);
    symlink(temp.path(), roots.user.join("ced")).unwrap();
    assert!(roots.create("ced", Tier::UserApp, "bad", "Bad").is_err());
    assert!(!temp.path().join("bad.mix").exists());
    let found = Catalogue::scan(&roots, "ced").unwrap();
    assert_eq!(found.entries.len(), 2);
    assert_eq!(found.diagnostics.len(), 2);
}

fn wait(runner: &Runner) -> Status {
    let deadline = Instant::now() + Duration::from_secs(10);
    while runner.status() == Status::Running {
        assert!(Instant::now() < deadline, "runner did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    runner.status()
}

#[test]
fn failed_start_is_reported_asynchronously() {
    let temp = tempfile::tempdir().unwrap();
    let mut roots = roots(temp.path());
    roots.mix = temp.path().join("absent");
    let entry = Entry {
        id: "test".into(),
        title: "Test".into(),
        tier: Tier::UserApp,
        path: temp.path().join("test.mix"),
        shadows: vec![],
    };
    let mut runner = Runner::default();
    runner.start(&roots, "ced", &entry).unwrap();
    assert!(matches!(wait(&runner), Status::FailedToStart(_)));
}

#[test]
fn real_mix_runs_with_context_bounded_output_and_stop() {
    let Some(mut roots) = Roots::resolve() else {
        eprintln!("skip: no absolute HOME");
        return;
    };
    if !roots.mix.is_file() {
        eprintln!("skip: real Mix binary absent at {}", roots.mix.display());
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    roots.home = temp.path().into();
    let path = temp.path().join("real.mix");
    fs::write(&path, "print(env(\"MIXOS_MACRO_APP\"))\nprint(env(\"MIXOS_MACRO_ID\"))\nprint(env(\"MIXOS_MACRO_TIER\"))\nprint(env(\"MIXOS_MACRO_PATH\"))\nprint(run_argv_must([\"pwd\"]))\n").unwrap();
    let entry = Entry {
        id: "real".into(),
        title: "Real".into(),
        tier: Tier::UserApp,
        path: path.clone(),
        shadows: vec![],
    };
    let mut runner = Runner::default();
    runner.start(&roots, "ced", &entry).unwrap();
    assert_eq!(wait(&runner), Status::Exited(Some(0)));
    let out = runner.output();
    assert!(out.text.contains("ced\nreal\nuser-app\n"), "{}", out.text);
    assert!(out.text.contains(path.to_str().unwrap()));
    assert!(out.text.contains(temp.path().to_str().unwrap()));
    fs::write(&path, "for $i = 1 to 10000\nprint(\"abcdefghij\")\nend\n").unwrap();
    runner.start(&roots, "ced", &entry).unwrap();
    assert_eq!(wait(&runner), Status::Exited(Some(0)));
    assert!(runner.output().dropped);
    assert!(runner.output().text.len() <= OUTPUT_LIMIT);
    fs::write(&path, "while true\nsleep(1)\nend\n").unwrap();
    runner.start(&roots, "ced", &entry).unwrap();
    assert!(runner.start(&roots, "ced", &entry).is_err());
    runner.stop();
    assert!(matches!(wait(&runner), Status::Exited(_)));
}
