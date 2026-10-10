// Fixture: constant arms the gate cannot resolve: a name declared nowhere, an import of a
// name the module does not declare, and a crate path to a missing name. Not compiled.
use crate::names::ALPHA_MISSING;

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_NOWHERE => 1,
        ALPHA_MISSING => 2,
        crate::names::ALPHA_ABSENT => 3,
        "alpha.ping" => 5,
        "alpha.other" => 6,
        "alpha.split" => 7,
        _ => 0,
    }
}
