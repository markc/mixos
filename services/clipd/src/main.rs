// SPDX-License-Identifier: MIT OR Apache-2.0
use clap::Parser;

fn main() -> anyhow::Result<()> {
    buildinfo::exit_on_version!();
    clipd::run(clipd::Args::parse())
}
