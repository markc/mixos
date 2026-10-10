// Fixture (round 24, decision 1): a glob import is the only place the bare ALPHA_PING arm can come
// from (this file has no const or explicit import of that name), so the name is unreadable (fail
// closed), not resolved. Not compiled.
use crate::names::*;

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_PING => 1,
        _ => 0,
    }
}
