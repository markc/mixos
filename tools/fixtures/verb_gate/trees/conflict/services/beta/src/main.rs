// Fixture: answers x.shared.verb, as does apps/alpha. Not compiled.

pub fn answer(verb: &str) -> u32 {
    match verb {
        "x.shared.verb" => 2,
        _ => 0,
    }
}
