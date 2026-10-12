// SPDX-License-Identifier: MIT OR Apache-2.0
//! `script_path()` through the real binary: the entry script's absolute
//! path is fixed when the script starts. A relative entry path is resolved
//! against the working directory at start, so a `chdir()` inside the script
//! does not move it, and neither does a prelude override that chdir()s
//! before the script starts. A symlinked entry answers with its target, the
//! same as `realpath($0)`. `-c` and stdin (`mix -`) answer nil.
//!
//! Out-of-process: the contract is about how the CLI sets up the entry, so
//! the CLI has to run.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mix_script_path_entry_{}_{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    dir
}

fn canonical_line(path: &Path) -> String {
    format!("{}\n", std::fs::canonicalize(path).unwrap().display())
}

fn run_in(dir: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_mix"))
        .current_dir(dir)
        .args(args)
        .env("MIX_STATS", "off")
        .output()
        .expect("failed to spawn mix binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "mix {args:?} exited non-zero\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

#[test]
fn relative_entry_path_is_resolved_before_a_chdir() {
    let dir = work_dir("chdir");
    let main = dir.join("main.mix");
    std::fs::write(&main, "chdir(\"sub\")\nprint(script_path())\n").unwrap();
    // The file is named relative to `dir`, which is the process cwd. The
    // script moves into `sub`; the answer must still be `dir/main.mix`.
    let out = run_in(&dir, &["main.mix"]);
    assert_eq!(out, canonical_line(&main));
}

#[test]
fn symlinked_entry_answers_with_its_target_like_realpath() {
    // script_path() and realpath($0) agree: the symlink's target, so a
    // script finds its siblings in the real tree.
    let dir = work_dir("symlink");
    let real = dir.join("real.mix");
    std::fs::write(&real, "print(script_path())\nprint(realpath($0))\n").unwrap();
    let link = dir.join("link.mix");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let out = run_in(&dir, &["link.mix"]);
    let expected = canonical_line(&real);
    assert_eq!(out, format!("{expected}{expected}"));
}

#[test]
fn relative_entry_path_survives_a_prelude_override_that_chdirs() {
    // A user prelude override runs before the entry script. When it chdir()s
    // away, the entry path must still be the absolute path it had at start.
    let dir = work_dir("prelude_chdir");
    let etc = dir.join("etc");
    std::fs::create_dir_all(&etc).unwrap();
    std::fs::write(etc.join("prelude.mix"), "chdir(\"/\")\n").unwrap();
    let main = dir.join("main.mix");
    std::fs::write(&main, "print(script_path())\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mix"))
        .current_dir(&dir)
        .arg("main.mix")
        .env("MIX_STATS", "off")
        .env("MIXOS_ETC", &etc)
        .output()
        .expect("failed to spawn mix binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "mix main.mix exited non-zero\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(stdout, canonical_line(&main));
}

#[test]
fn dash_c_has_no_entry_script() {
    let dir = work_dir("dash_c");
    let out = run_in(&dir, &["-c", "print(script_path())"]);
    assert_eq!(out, "nil\n");
}

#[test]
fn stdin_has_no_entry_script() {
    let dir = work_dir("stdin");
    let mut child = Command::new(env!("CARGO_BIN_EXE_mix"))
        .current_dir(&dir)
        .arg("-")
        .env("MIX_STATS", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn mix binary");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"print(script_path())\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "mix - failed");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "nil\n");
}
