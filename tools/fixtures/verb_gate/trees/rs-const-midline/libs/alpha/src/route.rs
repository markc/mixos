// Fixture (round 24): a literal arm, so the tree holds a registration of its own. Not compiled.
pub fn route(verb: &str) -> u8 {
    match verb {
        "alpha.ping" => 3,
        _ => 0,
    }
}
