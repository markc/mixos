// Fixture (round 24, decision 2): crate::names::ALPHA_PING reads names.rs's own definition. names.rs
// has a private glob import, which a path cannot reach, so it is ignored and the tree is clean.
// Not compiled.
pub fn run(verb: &str) -> u8 {
    match verb {
        crate::names::ALPHA_PING => 1,
        _ => 0,
    }
}
