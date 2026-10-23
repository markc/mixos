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

#[cfg(unix)]
#[test]
fn fifo_macro_is_diagnosed_without_blocking() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    fs::create_dir(&roots.user).unwrap();
    let path = roots.user.join("pipe.mix");
    let name = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: name is a valid C path; mkfifo creates only this test fixture.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(Catalogue::scan(&roots, "ced")).unwrap());
    let found = rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    assert!(found.entries.is_empty());
    assert_eq!(found.diagnostics.len(), 1);
    assert_eq!(found.diagnostics[0].path, path);
    assert!(found.diagnostics[0].message.contains("regular file"));
}

#[cfg(target_os = "linux")]
fn descendant_fixture(detached: bool) {
    let Some(mut roots) = Roots::resolve() else {
        eprintln!("skip: no absolute HOME");
        return;
    };
    if !roots.mix.is_file() {
        eprintln!("skip: real Mix binary absent");
        return;
    }
    if detached
        && !std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|p| p.join("setsid").is_file())
        })
    {
        eprintln!("skip: setsid absent");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    roots.home = temp.path().into();
    fs::write(temp.path().join("child.mix"),
        "write_file(\"child.pid\", to_string(pid()))\nwhile not exists(\"release\")\nsleep(0.01)\nend\nprint(\"old output\")\n").unwrap();
    let mix = format!("{:?}", roots.mix.to_str().unwrap());
    let argv = if detached {
        format!("[\"setsid\", {mix}, \"child.mix\"]")
    } else {
        format!("[{mix}, \"child.mix\"]")
    };
    let path = temp.path().join("parent.mix");
    fs::write(&path, format!(
        "$child = spawn({argv}, {{stdout: \"inherit\", stderr: \"inherit\"}})\nwhile not exists(\"child.pid\")\nsleep(0.01)\nend\n{}\n",
        if detached { "" } else { "while true\nsleep(1)\nend" }
    )).unwrap();
    let entry = Entry {
        id: "parent".into(), title: "Parent".into(), tier: Tier::UserApp,
        path, shadows: vec![],
    };
    let mut runner = Runner::default();
    runner.start(&roots, "ced", &entry).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid_file = temp.path().join("child.pid");
    while !pid_file.exists() {
        assert!(Instant::now() < deadline, "descendant did not start: {}", runner.output().text);
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid: u32 = fs::read_to_string(pid_file).unwrap().parse().unwrap();
    if detached {
        while runner.status() == Status::Running {
            assert!(Instant::now() < deadline, "detached pipe blocked completion");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(runner.status(), Status::Exited(Some(0)));
        fs::write(&entry.path, "sleep(0.3)\nprint(\"fresh output\")\n").unwrap();
        runner.start(&roots, "ced", &entry).unwrap();
        fs::write(temp.path().join("release"), "").unwrap();
        assert_eq!(wait(&runner), Status::Exited(Some(0)));
        assert_eq!(runner.output().text, "fresh output\n");
    } else {
        runner.stop();
        assert!(matches!(wait(&runner), Status::Exited(_)));
        // A killed grandchild can remain a zombie until init reaps it.
        let proc_stat = fs::read_to_string(format!("/proc/{pid}/stat"));
        assert!(proc_stat.is_err() || proc_stat.unwrap().split_once(") ").unwrap().1.starts_with('Z'));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn detached_pipes_do_not_block_completion_or_contaminate_the_next_run() {
    descendant_fixture(true);
}

#[cfg(target_os = "linux")]
#[test]
fn stop_terminates_the_macro_process_group() {
    descendant_fixture(false);
}
