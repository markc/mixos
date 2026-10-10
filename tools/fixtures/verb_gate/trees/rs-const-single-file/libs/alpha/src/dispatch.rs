// Fixture (round 24): a bare arm whose name is one file-level const and no import, so it resolves
// to that literal. Not compiled.
const ALPHA_NAME: &str = "alpha.ping";

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_NAME => 1,
        _ => 0,
    }
}
