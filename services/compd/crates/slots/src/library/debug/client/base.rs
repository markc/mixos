// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Every spawn goes through the hygiene wrapper — see `process.child`.
use crate::library::process::child::hygiene;
use crate::library::process::child::spawn as child_spawn;

pub fn spawn_client() {
    let mut args = std::env::args().skip(1);
    let flag = args.next();
    let arg = args.next();

    match (flag.as_deref(), arg) {
        (Some("-c") | Some("--command"), Some(program)) => {
            child_spawn::spawn_detached(&mut hygiene::command(program)).ok();
        }
        _ => {
            child_spawn::spawn_detached(&mut hygiene::command("foot")).ok();
        } // std::process::Command::new("weston-terminal").spawn().ok();        }
    }
}
