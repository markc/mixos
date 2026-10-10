// Fixture (round 24): a bare arm whose name is one import and no definition, so it resolves to the
// literal the imported module declares (names.rs). Not compiled.
use crate::names::ALPHA_NAME;

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_NAME => 1,
        _ => 0,
    }
}
