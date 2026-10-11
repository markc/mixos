// Fixture (round 29, ruling (c)): a glob import in a file that invokes NO trusted macro. Nothing
// the glob could shadow is in use here, so the rule does not fire and the round-24 behaviour holds:
// the local ALPHA_PING shadows the glob and the bare arm resolves to its literal. The tree is
// clean. Not compiled.
use crate::names::*;

const ALPHA_PING: &str = "alpha.ping";

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_PING => 1,
        _ => 0,
    }
}
