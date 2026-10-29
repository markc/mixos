// SPDX-License-Identifier: MIT OR Apache-2.0

#[test]
fn backend_probe_exits_before_config_or_display_initialisation() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_compd"))
        .arg("--backends")
        .env("COMPOSITOR_ENVIRONMENT", "invalid configuration")
        .env("WAYLAND_DISPLAY", "missing-probe-display")
        .env("DISPLAY", "invalid-probe-display")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let names: Vec<_> = [
        (cfg!(feature = "backend-native"), "native"),
        (cfg!(feature = "backend-winit"), "winit"),
        (cfg!(feature = "backend-headless"), "headless"),
    ]
    .into_iter()
    .filter_map(|(compiled, name)| compiled.then_some(name))
    .collect();
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        names.join(" ") + "\n"
    );
}
