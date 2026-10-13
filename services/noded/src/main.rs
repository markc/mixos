// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The standalone `noded` binary, for development builds. The daemon lives in
//! the `noded` library; the shipped `noded` is the system binary's personality.

use std::process::ExitCode;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    noded::main()
}
