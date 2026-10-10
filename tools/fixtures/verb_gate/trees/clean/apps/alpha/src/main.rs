// SPDX-License-Identifier: MIT OR Apache-2.0
// Fixture: every dispatch form the gate must read. Not compiled.

pub const VERBS: [&str; 2] = ["alpha.table", "alpha.wrapped"];

pub fn answer(verb: &str, command: &str, n: u32) -> u32 {
    let direct = match verb {
        "alpha.ping" => 1,
        "alpha.hello"
        | "alpha.hi" => 2,
        "alpha.table" => 7,
        _ => 0,
    };
    let tuple = match (n, command) {
        (0, "alpha.tuple") => 3,
        _ => 0,
    };
    let guarded = if verb == "alpha.guard" { 4 } else { 0 };
    let prefixed = match command {
        c if c.starts_with("alpha.sub.") => 5,
        _ => 0,
    };
    direct + tuple + guarded + prefixed
}

pub fn wrapped(verb: &str) -> Option<u32> {
    Some(match verb {
        "alpha.wrapped" => 6,
        _ => 0,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_in_tests() {
        match "alpha.test_only" {
            "alpha.test_only" => {}
            _ => {}
        }
    }
}
