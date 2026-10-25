// SPDX-License-Identifier: MIT OR Apache-2.0
use macros::{Catalogue, Entry, OUTPUT_LIMIT, Roots, Runner, Status, Tier, valid_id};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[path = "../../../tests/lib/mix_binary.rs"]
mod mix_binary;

fn roots(dir: &Path) -> Roots {
    Roots {
        builtin: dir.join("builtin"),
        user: dir.join("user"),
        home: dir.into(),
        mix: mix_binary::select()
            .expect("invalid test Mix selection")
            .unwrap_or_else(|| dir.join("missing-mix")),
    }
}

#[test]
fn workshop_management_preserves_ids_refuses_clashes_and_builtins() {
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    roots.create("ced", Tier::UserApp, "tidy", "Tidy").unwrap();
    roots
        .create("dopus", Tier::UserApp, "other", "Other")
        .unwrap();
    assert_eq!(roots.apps().unwrap(), ["ced", "dopus"]);
    let library = macros::Library::scan(&roots).unwrap();
    assert!(library.globals.entries.is_empty());
    assert_eq!(library.apps["ced"].entries[0].id, "tidy");
    assert_eq!(library.apps["dopus"].entries[0].id, "other");
    let mut entry = Catalogue::scan(&roots, "ced").unwrap().entries.remove(0);
    let before = roots.read(&entry).unwrap();
    let global = roots.move_to(&entry, Tier::UserGlobal, "ced").unwrap();
    assert!(!entry.path.exists());
    entry.path = global;
    entry.tier = Tier::UserGlobal;
    roots.create("ced", Tier::UserApp, "tidy", "Clash").unwrap();
    assert_eq!(
        roots
            .move_to(&entry, Tier::UserApp, "ced")
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(roots.read(&entry).unwrap(), before);
    let old = fs::File::open(&entry.path).unwrap();
    roots
        .write(&entry, "-- title: Changed\nprint(42)\n")
        .unwrap();
    use std::io::Read;
    let mut old_text = String::new();
    (&old).read_to_string(&mut old_text).unwrap();
    assert_eq!(
        old_text, before,
        "atomic replacement leaves an open reader intact"
    );
    assert!(roots.read(&entry).unwrap().contains("Changed"));
    assert_eq!(
        fs::read_dir(entry.path.parent().unwrap()).unwrap().count(),
        3
    );
    let mut builtin = entry.clone();
    builtin.tier = Tier::BuiltinGlobal;
    assert!(roots.write(&builtin, "bad").is_err());
    assert!(roots.remove(&builtin).is_err());
    assert!(roots.move_to(&builtin, Tier::UserApp, "ced").is_err());
    assert!(roots.move_to(&entry, Tier::BuiltinApp, "ced").is_err());
    roots.remove(&entry).unwrap();
    assert!(!entry.path.exists());
}

#[cfg(unix)]
#[test]
fn management_refuses_symlink_escape_without_touching_outside_file() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    roots.create("ced", Tier::UserApp, "safe", "Safe").unwrap();
    let mut entry = Catalogue::scan(&roots, "ced").unwrap().entries.remove(0);
    let outside = temp.path().join("outside.mix");
    fs::write(&outside, "original").unwrap();
    fs::remove_file(&entry.path).unwrap();
    symlink(&outside, &entry.path).unwrap();
    assert!(roots.read(&entry).is_err());
    assert!(roots.write(&entry, "bad").is_err());
    assert!(roots.remove(&entry).is_err());
    assert!(roots.move_to(&entry, Tier::UserGlobal, "ced").is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "original");
    entry.path = roots.user.join("ced");
    assert!(roots.write(&entry, "directory").is_err());
}

#[test]
fn target_headers_parse_and_refuse_before_acceptance() {
    use macros::{Session, Targets};
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    fs::create_dir_all(&roots.user).unwrap();
    for (id, header) in [
        ("any", "-- target: any\n"),
        ("list", "-- target: ced, dopus\n"),
        ("absent", ""),
        ("empty", "-- target:\n"),
        ("bad", "-- target: ced,,dopus\n"),
        ("duplicate", "-- target: any\n-- target: ced\n"),
    ] {
        fs::write(
            roots.user.join(format!("{id}.mix")),
            format!("-- title: {id}\n{header}print(1)\n"),
        )
        .unwrap();
    }
    let catalogue = Catalogue::scan_globals(&roots).unwrap();
    assert_eq!(catalogue.entries.len(), 3);
    assert_eq!(catalogue.diagnostics.len(), 3);
    assert_eq!(catalogue.entries[0].targets, None);
    assert_eq!(catalogue.entries[1].targets, Some(Targets::Any));
    assert_eq!(
        catalogue.entries[2].targets,
        Some(Targets::List(vec!["ced".into(), "dopus".into()]))
    );
    let session = Session::with_roots("workshop", Some(roots));
    session.reload().unwrap();
    for (id, target) in [("absent", "ced"), ("list", "term"), ("any", "../ced")] {
        assert_eq!(
            session.run_with(id, "bus", Some(target)).unwrap_err().code,
            "INVALID_ARGUMENT"
        );
        assert_eq!(session.status()["run"], serde_json::Value::Null);
    }
    let created = session.create("Global new").unwrap();
    assert_eq!(
        Path::new(created["path"].as_str().unwrap())
            .parent()
            .unwrap(),
        temp.path().join("user")
    );
}

#[test]
fn real_mix_receives_only_an_explicit_allowed_target() {
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    assert!(
        roots.mix.is_file(),
        "native Mix is required for target proof"
    );
    fs::create_dir_all(&roots.user).unwrap();
    fs::write(
        roots.user.join("target.mix"),
        "-- target: ced, dopus\nprint(env(\"MIXOS_MACRO_TARGET\", \"absent\"))\n",
    )
    .unwrap();
    let entry = Catalogue::scan_globals(&roots).unwrap().entries.remove(0);
    let mut runner = Runner::default();
    runner
        .start_with(&roots, "workshop", &entry, Some("ced"))
        .unwrap();
    assert_eq!(wait(&runner), Status::Exited(Some(0)));
    assert_eq!(runner.output().text.trim(), "ced");
    runner.start(&roots, "workshop", &entry).unwrap();
    assert_eq!(wait(&runner), Status::Exited(Some(0)));
    assert_eq!(runner.output().text.trim(), "absent");
}

#[test]
fn roots_are_pure_absolute_and_injectable() {
    let home = PathBuf::from("/home/user");
    let fixture = tempfile::tempdir().unwrap();
    let prefix = fixture.path().join("fixture prefix");
    let resolved = Roots::resolve_with(|key| match key {
        "HOME" => Some(home.clone()),
        "MIXOS_PREFIX" => Some(prefix.clone()),
        _ => Some("relative".into()),
    })
    .unwrap();
    assert_eq!(resolved.builtin, prefix.join("share/macros"));
    assert_eq!(resolved.user, home.join(".config/mixos/macros"));
    assert_eq!(resolved.mix, prefix.join("bin/mix"));
    let resolved = Roots::resolve_with(|key| {
        if key == "MIXOS_PREFIX" {
            return Some(fixture.path().join("another prefix"));
        }
        Some(PathBuf::from(match key {
            "HOME" => "/home/user",
            "MIXOS_SHARE" => "/share",
            "XDG_CONFIG_HOME" => "/config",
            "MIXOS_MIX" => "/bin/custom-mix",
            _ => return None,
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
    let folder = roots.user.join("ced");
    fs::write(folder.join("z.mix"), "print(1)").unwrap();
    fs::write(folder.join(".hidden.mix"), "").unwrap();
    fs::write(folder.join("Bad.mix"), "").unwrap();
    fs::write(folder.join("document.mx"), "").unwrap();
    fs::create_dir(folder.join("directory.mix")).unwrap();
    let found = Catalogue::scan(&roots, "ced").unwrap();
    let ids: Vec<_> = found.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["builtin-app", "shared", "user-app", "z"]);
    let shared = &found.entries[1];
    assert_eq!(shared.tier, Tier::UserApp);
    assert_eq!(shared.title, "user-app");
    assert_eq!(shared.shadows, [Tier::BuiltinApp]);
    assert_eq!(found.entries[3].title, "z");
    assert_eq!(found.diagnostics.len(), 3);
    // Removing each winner exposes exactly the next tier.
    for expected in [Tier::UserApp, Tier::BuiltinApp] {
        let found = Catalogue::scan(&roots, "ced").unwrap();
        let entry = found.entries.iter().find(|e| e.id == "shared").unwrap();
        assert_eq!(entry.tier, expected);
        fs::remove_file(&entry.path).unwrap();
    }
    assert!(Catalogue::scan(&roots, "../escape").is_err());
    let globals = Catalogue::scan_globals(&roots).unwrap();
    let shared = globals.entries.iter().find(|e| e.id == "shared").unwrap();
    assert_eq!(shared.tier, Tier::UserGlobal);
    assert_eq!(shared.shadows, [Tier::BuiltinGlobal]);
    assert!(globals.entries.iter().all(|e| !e.tier.is_app()));
    assert!(Catalogue::scan(&roots, "dopus").unwrap().entries.is_empty());
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
            targets: None,
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

#[test]
fn create_from_title_slugs_and_allocates_without_overwriting() {
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    for (title, expected) in [
        ("Tidy imports", "tidy-imports.mix"),
        ("Tidy imports", "tidy-imports-2.mix"),
        ("Tidy imports", "tidy-imports-3.mix"),
        ("  A___B!? C  ", "a-b-c.mix"),
        ("日本語", "macro.mix"),
    ] {
        let path = roots.create_from_title("ced", title).unwrap();
        assert_eq!(path.file_name().unwrap(), expected);
        assert!(
            fs::read_to_string(path)
                .unwrap()
                .starts_with(&format!("-- title: {title}\n"))
        );
    }
    let original = roots.user.join("ced/tidy-imports.mix");
    let before = fs::read(&original).unwrap();
    assert!(
        roots
            .create("ced", Tier::UserApp, "tidy-imports", "Other")
            .is_err()
    );
    assert_eq!(fs::read(original).unwrap(), before);
    for title in ["", "  ", "Injected\nprint(1)", "Injected\rtitle"] {
        assert!(roots.create_from_title("ced", title).is_err());
    }
}

#[cfg(unix)]
#[test]
fn create_from_title_counts_dangling_symlinks_as_name_clashes() {
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    let folder = roots.ensure_user_folder("ced", Tier::UserApp).unwrap();
    let occupied = folder.join("tidy.mix");
    std::os::unix::fs::symlink(folder.join("missing.mix"), &occupied).unwrap();
    let path = roots.create_from_title("ced", "Tidy").unwrap();
    assert_eq!(path.file_name().unwrap(), "tidy-2.mix");
    assert!(
        occupied
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn create_from_title_reports_obstructed_parents_without_retrying() {
    for obstruct_root in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let roots = roots(temp.path());
        let obstruction = if obstruct_root {
            roots.user.clone()
        } else {
            fs::create_dir(&roots.user).unwrap();
            roots.user.join("ced")
        };
        fs::write(&obstruction, "keep").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(roots.create_from_title("ced", "Tidy imports"));
        });
        let result = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("parent obstruction must report an error, not retry every slug");
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(obstruction).unwrap(), "keep");
    }
}

#[cfg(unix)]
#[test]
fn macro_symlinks_are_refused_even_inside_the_root() {
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
    let found = Catalogue::scan_globals(&roots).unwrap();
    assert_eq!(found.entries.len(), 1);
    assert_eq!(found.diagnostics.len(), 2);
    symlink(temp.path(), roots.user.join("ced")).unwrap();
    assert!(roots.create("ced", Tier::UserApp, "bad", "Bad").is_err());
    assert!(!temp.path().join("bad.mix").exists());
    let found = Catalogue::scan(&roots, "ced").unwrap();
    assert_eq!(found.entries.len(), 0);
    assert_eq!(found.diagnostics.len(), 1);
}

#[cfg(unix)]
#[test]
fn app_discovery_skips_vanished_entries_and_keeps_other_apps() {
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    fs::create_dir_all(roots.user.join("ced")).unwrap();
    // A directory entry whose target has vanished reproduces canonicalize's
    // NotFound branch deterministically, without a scheduler-dependent race.
    std::os::unix::fs::symlink(roots.user.join("gone"), roots.user.join("vanished")).unwrap();
    assert_eq!(roots.apps().unwrap(), ["ced"]);
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
        targets: None,
    };
    let mut runner = Runner::default();
    runner.start(&roots, "ced", &entry).unwrap();
    assert!(matches!(wait(&runner), Status::FailedToStart(_)));
}

#[test]
fn real_mix_runs_with_context_bounded_output_and_stop() {
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    if !mix_binary::executable(&roots.mix) {
        eprintln!(
            "SKIP macro runner: no executable mix on PATH; set MIXOS_E2E_MIX_BIN to select one"
        );
        return;
    }
    let path = temp.path().join("real.mix");
    fs::write(&path, "print(env(\"MIXOS_MACRO_APP\"))\nprint(env(\"MIXOS_MACRO_ID\"))\nprint(env(\"MIXOS_MACRO_TIER\"))\nprint(env(\"MIXOS_MACRO_PATH\"))\nprint(run_argv_must([\"pwd\"]))\n").unwrap();
    let entry = Entry {
        id: "real".into(),
        title: "Real".into(),
        tier: Tier::UserApp,
        path: path.clone(),
        shadows: vec![],
        targets: None,
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
    std::thread::spawn(move || tx.send(Catalogue::scan_globals(&roots)).unwrap());
    let found = rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    assert!(found.entries.is_empty());
    assert_eq!(found.diagnostics.len(), 1);
    assert_eq!(found.diagnostics[0].path, path);
    assert!(found.diagnostics[0].message.contains("regular file"));
}

#[cfg(target_os = "linux")]
fn descendant_fixture(detached: bool) {
    let temp = tempfile::tempdir().unwrap();
    let roots = roots(temp.path());
    if !mix_binary::executable(&roots.mix) {
        eprintln!(
            "SKIP macro descendants: no executable mix on PATH; set MIXOS_E2E_MIX_BIN to select one"
        );
        return;
    }
    if detached
        && !std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|p| p.join("setsid").is_file()))
    {
        eprintln!("skip: setsid absent");
        return;
    }
    fs::write(temp.path().join("child.mix"),
        "write_file(\"child.pid\", to_string(pid()))\n$deadline = time() + 30\nwhile not exists(\"release\") and time() < $deadline\nsleep(0.01)\nend\nprint(\"old output\")\n").unwrap();
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
        id: "parent".into(),
        title: "Parent".into(),
        tier: Tier::UserApp,
        path,
        shadows: vec![],
        targets: None,
    };
    let mut runner = Runner::default();
    runner.start(&roots, "ced", &entry).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid_file = temp.path().join("child.pid");
    while !pid_file.exists() {
        assert!(
            Instant::now() < deadline,
            "descendant did not start: {}",
            runner.output().text
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid: u32 = fs::read_to_string(pid_file).unwrap().parse().unwrap();
    if detached {
        while runner.status() == Status::Running {
            assert!(
                Instant::now() < deadline,
                "detached pipe blocked completion"
            );
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
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let proc_stat = fs::read_to_string(format!("/proc/{pid}/stat"));
            if proc_stat.is_err()
                || proc_stat
                    .unwrap()
                    .split_once(") ")
                    .unwrap()
                    .1
                    .starts_with('Z')
            {
                break;
            }
            assert!(Instant::now() < deadline, "macro child survived stop");
            std::thread::sleep(Duration::from_millis(10));
        }
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
