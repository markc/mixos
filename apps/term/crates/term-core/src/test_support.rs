// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real PTY tests use production self-location with an explicit child prefix.
#[path = "../../../../../tests/lib/mix_binary.rs"]
pub(crate) mod mix_binary;

use std::path::PathBuf;
use std::process::Command;

const CHILD: &str = "MIXOS_TERM_PTY_TEST";

struct Prefix(PathBuf);
impl Drop for Prefix {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run the named libtest once with a private prefix/bin/mix pointing to the
/// explicit MIXOS_E2E_MIX_BIN or PATH selection. Only that child runs the body.
/// No process-wide environment is mutated, including in parallel tests.
pub fn with_mix(test: &str) -> bool {
    if std::env::var(CHILD).ok().as_deref() == Some(test) {
        let prefix = std::env::var_os("MIXOS_PREFIX").expect("PTY child needs an explicit prefix");
        let program = PathBuf::from(prefix).join("bin/mix");
        assert_eq!(::paths::bin("mix"), program, "guard and production launcher must select the same Mix");
        assert!(mix_binary::executable(&program), "PTY target is not executable: {}", program.display());
        eprintln!("PTY fixture {test}: {}", program.display());
        return true;
    }
    let Some(mix) = mix_binary::select().expect("invalid test Mix selection") else {
        eprintln!("SKIP {test}: no executable mix on PATH; set MIXOS_E2E_MIX_BIN to select one");
        return false;
    };
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("mixos-term-pty-{}-{serial}", std::process::id()));
    std::fs::create_dir(&root).expect("create owned PTY prefix");
    let root = Prefix(root);
    std::fs::create_dir(root.0.join("bin")).unwrap();
    std::os::unix::fs::symlink(&mix, root.0.join("bin/mix")).unwrap();
    for dir in ["home", "etc", "var", "run", "cache", "xdg-config", "xdg-data", "xdg-cache"] {
        std::fs::create_dir(root.0.join(dir)).unwrap();
    }
    let share = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../share");
    std::os::unix::fs::symlink(share, root.0.join("share")).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--include-ignored", "--nocapture"])
        .env(CHILD, test)
        .env("MIXOS_PREFIX", &root.0)
        .env_remove("MIXOS")
        .env_remove("TERM_CWD")
        .env("HOME", root.0.join("home"))
        .env("MIXOS_ETC", root.0.join("etc"))
        .env("MIXOS_VAR", root.0.join("var"))
        .env("MIXOS_RUN", root.0.join("run"))
        .env("MIXOS_CACHE", root.0.join("cache"))
        .env("MIXOS_SHARE", root.0.join("share"))
        .env("XDG_CONFIG_HOME", root.0.join("xdg-config"))
        .env("XDG_DATA_HOME", root.0.join("xdg-data"))
        .env("XDG_RUNTIME_DIR", root.0.join("run"))
        .env("XDG_CACHE_HOME", root.0.join("xdg-cache"))
        .env("RUST_TEST_THREADS", "1")
        .env("RUST_TEST_NOCAPTURE", "1")
        .output()
        .expect("run PTY test child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success()
            && stdout.contains("test result: ok. 1 passed")
            && stderr.contains(&format!("PTY fixture {test}:")),
        "PTY test child failed for {test} using {}:\n{stdout}\n{stderr}", mix.display()
    );
    false
}
