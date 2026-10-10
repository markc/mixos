// Fixture: alpha answers its own verb, alpha.ping. The listed test double, libs/beta/src/fake.rs,
// is absent from this tree. Not compiled.

pub fn route(verb: &str) -> u32 {
    match verb {
        "alpha.ping" => 1,
        _ => 0,
    }
}
