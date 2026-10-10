// Fixture: a listed test double that answers alpha.ping, which alpha also answers. It is scanned
// in full, and its answer is not an owner, so there is no owner-conflict. Not compiled.

pub fn route(verb: &str) -> u32 {
    match verb {
        "alpha.ping" => 1,
        _ => 0,
    }
}
