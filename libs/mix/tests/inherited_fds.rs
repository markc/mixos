// SPDX-License-Identifier: MIT OR Apache-2.0
//! A program started by a structured spawn builtin never sees a descriptor
//! this process inherited without close-on-exec (mix::fd_hygiene). CI runners
//! hand such descriptors out (GitHub's leaves fds 142 and 145 open), so the
//! fixture plants one the same way: a dup above 200 with close-on-exec clear.

#![cfg(target_os = "linux")]

use mix::evaluator::{Evaluator, SharedBuf};
use mix::lexer::Lexer;
use mix::parser::Parser;
use std::os::fd::AsRawFd;

async fn run(source: &str) -> String {
    let tokens = Lexer::new(source).tokenize().unwrap();
    let stmts = Parser::new(tokens, source).parse_program().unwrap();
    let stdout = SharedBuf::new();
    let stderr = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(stdout.clone()), Box::new(stderr.clone()));
    if let Err(e) = eval.execute(&stmts).await {
        panic!("script failed: {e}; stderr={}", stderr.to_string_lossy());
    }
    stdout.to_string_lossy()
}

fn listed(text: &str) -> Vec<i32> {
    text.split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect()
}

#[tokio::test]
async fn spawned_programs_never_see_inherited_descriptors() {
    let file = std::fs::File::open("/dev/null").unwrap();
    // F_DUPFD, not F_DUPFD_CLOEXEC: the copy is inheritable, as a stray
    // descriptor from a parent would be.
    let stray = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 200) };
    assert!(stray >= 200, "fixture: no inheritable descriptor");
    let dir = std::env::temp_dir().join(format!("mix-inherited-fds-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = |name: &str| format!("{:?}", dir.join(name).to_string_lossy());

    // The fixture is real: a plain child of this test process sees it.
    let plain = std::process::Command::new("ls")
        .arg("/proc/self/fd")
        .output()
        .unwrap();
    assert!(
        listed(&String::from_utf8_lossy(&plain.stdout)).contains(&stray),
        "fixture: an unsealed child must see fd {stray}"
    );

    let report = run(&format!(
        "$a = run_argv([\"ls\", \"/proc/self/fd\"])\n\
         print(\"argv \" .. replace($a.stdout, \"\\n\", \" \"))\n\
         $p = run_pipeline([[\"ls\", \"/proc/self/fd\"], [\"cat\"]])\n\
         print(\"pipeline \" .. replace($p.stdout, \"\\n\", \" \"))\n\
         $s = run_stream([\"sh\", \"-c\", \"ls /proc/self/fd > \" .. {stream}])\n\
         $pid = spawn([\"sh\", \"-c\", \"ls /proc/self/fd > \" .. {spawn_argv}])\n\
         $pid2 = spawn(\"ls /proc/self/fd > \" .. {spawn_sh})\n\
         $pid3 = spawn([\"sh\", \"-c\", \"ls /proc/self/fd > \" .. {detached}], {{detach: true}})\n",
        stream = out("stream"),
        spawn_argv = out("spawn_argv"),
        spawn_sh = out("spawn_sh"),
        detached = out("detached"),
    ))
    .await;
    for line in report.lines() {
        let (route, fds) = line.split_once(' ').unwrap();
        assert!(
            !listed(fds).contains(&stray),
            "{route}: the child saw inherited fd {stray}: {fds}"
        );
    }

    // spawn returns at once: wait for each listing to be written.
    for route in ["stream", "spawn_argv", "spawn_sh", "detached"] {
        let path = dir.join(route);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let text = loop {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if !text.is_empty() {
                    break text;
                }
            }
            assert!(std::time::Instant::now() < deadline, "{route}: no listing");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(
            !listed(&text).contains(&stray),
            "{route}: the child saw inherited fd {stray}: {text}"
        );
    }
    unsafe { libc::close(stray) };
    let _ = std::fs::remove_dir_all(&dir);
}
