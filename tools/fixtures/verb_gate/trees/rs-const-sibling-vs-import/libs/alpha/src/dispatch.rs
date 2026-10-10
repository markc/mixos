// Fixture (round 24): a fn-local ALPHA_NAME in a sibling fn, and an imported ALPHA_NAME. The bare
// arm has two candidates, so it is ambiguous-constant (fail closed); the sibling fn's local does
// not win over the import. Not compiled.
use crate::names::ALPHA_NAME;

fn helper() -> &'static str {
    const ALPHA_NAME: &str = "alpha.ping";
    ALPHA_NAME
}

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_NAME => 1,
        _ => 0,
    }
}
