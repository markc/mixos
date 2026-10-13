// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `mix lint` `handlers` field: every `on <command>` declaration the native
//! parser built, reported as structural evidence. Lint never runs a fixture,
//! so each fixture writes a sentinel file that must still be absent after
//! the lint.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

/// Run `mix lint` with `args` and return the raw process output.
fn lint(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mix"))
        .arg("lint")
        .args(args)
        .output()
        .expect("spawn mix lint")
}

/// Run `mix lint --json` over `paths`; returns the exit code and parsed report.
fn lint_json(paths: &[&Path]) -> (Option<i32>, Value) {
    let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    let mut args = vec!["--json"];
    args.extend(names.iter().map(String::as_str));
    let out = lint(&args);
    let report = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}); status {:?}\nstderr: {}\nstdout: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (out.status.code(), report)
}

/// Write `source` to `name` inside `dir` and return the path.
fn write(dir: &TempDir, name: &str, source: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, source).expect("write fixture");
    path
}

/// The report's `handlers` array as `(command, file, line)` triples.
fn handlers(report: &Value) -> Vec<(String, Option<String>, u64)> {
    report["handlers"]
        .as_array()
        .expect("handlers is always an array")
        .iter()
        .map(|h| {
            (
                h["command"].as_str().expect("command is a string").to_string(),
                h["file"].as_str().map(str::to_string),
                h["line"].as_u64().expect("line is a number"),
            )
        })
        .collect()
}

fn decl(command: &str, file: &Path, line: u64) -> (String, Option<String>, u64) {
    (
        command.to_string(),
        Some(file.display().to_string()),
        line,
    )
}

/// Every 1-based line of `source` containing `needle`, in order.
fn lines_of(source: &str, needle: &str) -> Vec<u64> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(needle))
        .map(|(i, _)| i as u64 + 1)
        .collect()
}

/// The diagnostic codes in the report.
fn codes(report: &Value) -> Vec<String> {
    report["diagnostics"]
        .as_array()
        .expect("diagnostics is an array")
        .iter()
        .filter_map(|d| d["code"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn examples_in_comments_strings_and_heredocs_declare_nothing() {
    let dir = TempDir::new().expect("tempdir");
    let source = r#"# on comment.example
$single = 'on single.quoted'
$double = "on double.quoted"
$escaped = "say \"on escaped.quoted\" now"
$sample = "${"on @.call"}"
$doc = <<END
on heredoc.example
  print("not code")
done
END
$multi = "line one
on multiline.example
done"
"#;
    let path = write(&dir, "examples.mix", source);
    let (_, report) = lint_json(&[path.as_path()]);

    let codes = codes(&report);
    assert!(
        !codes.iter().any(|c| c.starts_with("MIX-E100")),
        "fixture must parse as a script: {codes:?}"
    );
    assert!(
        report["strict_data_files"].as_array().unwrap().is_empty(),
        "fixture must not fall back to strict data: {report}"
    );
    assert_eq!(handlers(&report), Vec::new(), "report: {report}");
}

#[test]
fn ssh_mix_remote_body_declares_nothing_locally() {
    let dir = TempDir::new().expect("tempdir");
    let source = r#"$body = <<END
on remote.body
  print("remote")
done
END
$r = ssh_mix("example-host", $body)
"#;
    let path = write(&dir, "remote.mix", source);
    let (_, report) = lint_json(&[path.as_path()]);
    assert_eq!(handlers(&report), Vec::new(), "report: {report}");
}

#[test]
fn top_level_on_reports_command_file_and_line() {
    let dir = TempDir::new().expect("tempdir");
    let source = r#"$ready = 1
on ping.one
  print("pong")
end
"#;
    let path = write(&dir, "ping.mix", source);
    let (_, report) = lint_json(&[path.as_path()]);
    let lines = lines_of(source, "on ping.one");
    assert_eq!(
        handlers(&report),
        vec![decl("ping.one", &path, lines[0])],
        "report: {report}"
    );
}

#[test]
fn top_level_instance_prefix_reports_at_command() {
    let dir = TempDir::new().expect("tempdir");
    let source = r#"$ready = 1
on @.call desc "fixture"
  print("never runs")
end
"#;
    let path = write(&dir, "call.mix", source);
    let (_, report) = lint_json(&[path.as_path()]);
    let lines = lines_of(source, "on @.call");
    assert_eq!(
        handlers(&report),
        vec![decl("@.call", &path, lines[0])],
        "report: {report}"
    );
}

/// The walk visits statement bodies before expressions, so the order of these
/// declarations is the walk's traversal order. It is not a claim that every
/// nested declaration comes out in source order.
#[test]
fn nested_real_declarations_are_collected_in_traversal_order() {
    let dir = TempDir::new().expect("tempdir");
    let source = r#"fn register_inner()
  on inner.fn
    print("inner")
  end
end
$handler = fn($x)
  on lambda.block
    print("lambda")
  end
end
$picked = if true then
  on expr.branch
    print("branch")
  end
  1
else
  2
end
if true then
  on control.block
    print("control")
  end
end
"#;
    let path = write(&dir, "nested.mix", source);
    let (_, report) = lint_json(&[path.as_path()]);
    assert_eq!(
        handlers(&report),
        vec![
            decl("inner.fn", &path, lines_of(source, "on inner.fn")[0]),
            decl("lambda.block", &path, lines_of(source, "on lambda.block")[0]),
            decl("expr.branch", &path, lines_of(source, "on expr.branch")[0]),
            decl("control.block", &path, lines_of(source, "on control.block")[0]),
        ],
        "report: {report}"
    );
}

#[test]
fn duplicate_declarations_are_kept_in_order() {
    let dir = TempDir::new().expect("tempdir");
    let source = r#"on dup.cmd
  print("first")
end
on dup.cmd
  print("second")
end
"#;
    let path = write(&dir, "dup.mix", source);
    let (_, report) = lint_json(&[path.as_path()]);
    let lines = lines_of(source, "on dup.cmd");
    assert_eq!(
        handlers(&report),
        vec![
            decl("dup.cmd", &path, lines[0]),
            decl("dup.cmd", &path, lines[1]),
        ],
        "report: {report}"
    );
}

#[test]
fn lint_does_not_run_the_fixture() {
    let dir = TempDir::new().expect("tempdir");
    let sentinel = dir.path().join("sentinel.txt");
    let source = format!(
        "write_file(\"{}\", \"ran\")\non guarded.cmd\n  print(\"never runs\")\nend\n",
        sentinel.display()
    );
    let path = write(&dir, "guarded.mix", &source);
    let (_, report) = lint_json(&[path.as_path()]);

    let lines = lines_of(&source, "on guarded.cmd");
    assert_eq!(
        handlers(&report),
        vec![decl("guarded.cmd", &path, lines[0])],
        "report: {report}"
    );
    assert!(
        !sentinel.exists(),
        "lint executed the fixture: {} was written",
        sentinel.display()
    );
}

#[test]
fn handlers_are_attributed_to_each_input_file() {
    let dir = TempDir::new().expect("tempdir");
    let source_a = "on alpha.cmd\n  print(\"a\")\nend\n";
    let source_b = "$plain = 2\non beta.cmd\n  print(\"b\")\nend\n";
    let path_a = write(&dir, "a.mix", source_a);
    let path_b = write(&dir, "b.mix", source_b);
    let (_, report) = lint_json(&[path_a.as_path(), path_b.as_path()]);
    assert_eq!(
        handlers(&report),
        vec![
            decl("alpha.cmd", &path_a, 1),
            decl("beta.cmd", &path_b, lines_of(source_b, "on beta.cmd")[0]),
        ],
        "report: {report}"
    );
}

#[test]
fn strict_data_input_reports_empty_handlers() {
    let dir = TempDir::new().expect("tempdir");
    let path = write(&dir, "settings.conf.mix", "\"name\": \"demo\"\n");
    let (code, report) = lint_json(&[path.as_path()]);
    assert_eq!(code, Some(0), "report: {report}");
    assert_eq!(
        report["strict_data_files"][0].as_str(),
        Some(path.display().to_string().as_str()),
        "report: {report}"
    );
    assert_eq!(handlers(&report), Vec::new(), "report: {report}");
}

#[test]
fn invalid_source_fails_and_forges_no_handler() {
    let dir = TempDir::new().expect("tempdir");
    let path = write(&dir, "broken.mix", "on forged.cmd\n  print(\"x\"\nend\n");
    let (code, report) = lint_json(&[path.as_path()]);
    assert_eq!(code, Some(1), "report: {report}");
    assert!(
        codes(&report).iter().any(|c| c == "MIX-E1002"),
        "expected a script parse error: {report}"
    );
    assert_eq!(handlers(&report), Vec::new(), "report: {report}");
}

#[test]
fn data_output_carries_handlers() {
    let dir = TempDir::new().expect("tempdir");
    let source = "on ping.one\n  print(\"pong\")\nend\n";
    let path = write(&dir, "ping.mix", source);
    let path_str = path.to_str().expect("utf-8 temp path");
    let out = lint(&["--data", path_str]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("handlers"), "--data output lacks handlers: {text}");
    assert!(text.contains("ping.one"), "--data output lacks the command: {text}");
    mix::parse_data(&text).unwrap_or_else(|e| panic!("--data output is not strict data ({e}):\n{text}"));
}
