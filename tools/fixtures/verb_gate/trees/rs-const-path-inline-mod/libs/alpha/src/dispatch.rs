// Fixture (round 25): the inline-module path reads the module's own ALPHA_PING. Not compiled.
pub fn run(verb: &str) -> u8 {
    match verb {
        crate::names::inner::ALPHA_PING => 1,
        _ => 0,
    }
}
