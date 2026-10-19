// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Schema 3 function metadata must survive CLI aggregation and data rendering.

use std::process::Command;

#[test]
fn schema_three_functions_are_attributed_without_execution() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("context.mix");
    let data = dir.path().join("settings.conf.mix");
    let broken = dir.path().join("broken.mix");
    let sentinel = dir.path().join("must-not-exist");
    std::fs::write(
        &source,
        format!(
            "fn esc($x) = $x\nwrite_file({:?}, \"ran\")\n$quoted = 'fn forged() end'\n",
            sentinel.to_str().unwrap()
        ),
    )
    .unwrap();
    std::fs::write(&data, "name: \"demo\"\n").unwrap();
    std::fs::write(&broken, "fn broken(\n").unwrap();
    for format in ["--json", "--data"] {
        let out = Command::new(env!("CARGO_BIN_EXE_mix"))
            .args(["lint", format])
            .args([&source, &data, &broken])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        let text = String::from_utf8(out.stdout).unwrap();
        let report: serde_json::Value = if format == "--json" {
            serde_json::from_str(&text).unwrap()
        } else {
            mix::from_value(&mix::parse_data(&text).unwrap()).unwrap()
        };
        // Strict data has one numeric type (f64), unlike JSON's integer type.
        assert_eq!(report["schema_version"].as_f64(), Some(3.0));
        let functions = report["functions"].as_array().unwrap();
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0]["name"], "esc");
        assert_eq!(functions[0]["file"], source.to_str().unwrap());
        assert_eq!(functions[0]["line"].as_f64(), Some(1.0));
        for key in [
            "tool",
            "mix_version",
            "files",
            "strict_data_files",
            "diagnostics",
            "capabilities",
            "handlers",
            "summary",
        ] {
            assert!(report.get(key).is_some(), "missing existing key {key}");
        }
        assert!(!sentinel.exists());
    }
}
