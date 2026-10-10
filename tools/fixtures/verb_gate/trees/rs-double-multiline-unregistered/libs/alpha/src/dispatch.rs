// Fixture: alpha answers its own verb, alpha.ping. Not compiled.

pub fn route(verb: &str) -> u32 {
    match verb {
        "alpha.ping" => 1,
        _ => 0,
    }
}
