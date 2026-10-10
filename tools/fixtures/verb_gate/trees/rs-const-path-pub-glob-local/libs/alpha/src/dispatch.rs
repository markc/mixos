// Fixture (round 24, decision 2): names.rs re-exports a glob and also defines ALPHA_PING. The own
// definition shadows the glob, so the path resolves to its literal and the tree is clean. Not
// compiled.
pub fn run(verb: &str) -> u8 {
    match verb {
        crate::names::ALPHA_PING => 1,
        _ => 0,
    }
}
