// Fixture: constant arms that resolve to their literals, three ways: a const declared in
// this file, one imported by a plain use, and one named by a crate path. Not compiled.
use crate::names::ALPHA_OTHER;

const ALPHA_PING: &str = "alpha.ping";

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_PING => 1,
        ALPHA_OTHER => 2,
        crate::names::ALPHA_SPLIT => 4,
        _ => 0,
    }
}
