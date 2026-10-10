// Fixture (round 24): a bare ALPHA_NAME arm where an impl declares its own ALPHA_NAME and the file
// imports another ALPHA_NAME. The bare name has two candidates, so it is ambiguous-constant (fail
// closed); the impl's const does not win by scope. Not compiled.
use crate::names::ALPHA_NAME;

pub struct First;

impl First {
    const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        match verb {
            ALPHA_NAME => 1,
            _ => 0,
        }
    }
}
