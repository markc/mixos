// SPDX-License-Identifier: MIT OR Apache-2.0
// Fixture: answers the edit.ping verb declared in libs/edit. Not compiled.

pub fn route(verb: &str) -> u32 {
    match verb {
        "edit.ping" => 1,
        _ => 0,
    }
}
