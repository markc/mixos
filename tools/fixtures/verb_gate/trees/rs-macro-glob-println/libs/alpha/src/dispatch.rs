// Fixture (round 29, ruling (c)): a glob import in a file that INVOKES a trusted macro. A glob can
// import a macro named println, and a glob shadows the prelude, so the println! below could be the
// glob's macro. The file's constants are not read, so the bare ALPHA_PING arm is unreadable (fail
// closed), although a local const of that name exists. Not compiled.
use crate::names::*;

const ALPHA_PING: &str = "alpha.ping";

pub fn run(verb: &str) -> u8 {
    println!("run");
    match verb {
        ALPHA_PING => 1,
        _ => 0,
    }
}
