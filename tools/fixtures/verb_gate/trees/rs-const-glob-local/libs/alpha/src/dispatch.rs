// Fixture (round 24, decision 1): a glob import beside a file-level const of the same name. The
// local item shadows the glob, as in Rust, so the bare ALPHA_PING arm resolves to its literal and
// the tree is clean. Not compiled.
use crate::names::*;

const ALPHA_PING: &str = "alpha.ping";

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_PING => 1,
        _ => 0,
    }
}
