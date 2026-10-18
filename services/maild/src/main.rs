// SPDX-License-Identifier: MIT OR Apache-2.0
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> std::process::ExitCode {
    maild::main()
}
