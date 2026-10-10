// Fixture: a test double that answers alpha.ping, as a fake editd does. Listed in the
// exemptions as a test double, so it registers nothing. Not compiled.

pub fn route(verb: &str) -> u32 {
    match verb {
        "alpha.ping" => 1,
        _ => 0,
    }
}
