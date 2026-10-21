// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The real ced entry point with a test-only log subscriber. Production ced
//! has no logger, so RUST_LOG alone cannot expose eframe's chosen AdapterInfo.
#[path = "../src/main.rs"]
mod program;

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("warn,egui_wgpu=debug,wgpu_core=info,eframe=info")
        .with_ansi(false)
        .init();
    program::main();
}
