// Fixture: a listed test double that answers alpha.ping (registered) and beta.ghost (not
// registered). It is scanned in full, so beta.ghost is unregistered. Not compiled.

pub fn route(verb: &str) -> u32 {
    match verb {
        "alpha.ping" => 1,
        "beta.ghost" => 2,
        _ => 0,
    }
}
