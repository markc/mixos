// Fixture (round 26): ALPHA_NAME is only an impl-level associated constant, which is not in bare
// scope, and the file has a glob import that may supply the name. The bare arm is unreadable (fail
// closed), never the impl literal. Not compiled.
use crate::other::*;

pub struct Alpha;

impl Alpha {
    const ALPHA_NAME: &'static str = "alpha.ping";
}

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_NAME => 1,
        "alpha.ping" => 2,
        _ => 0,
    }
}
