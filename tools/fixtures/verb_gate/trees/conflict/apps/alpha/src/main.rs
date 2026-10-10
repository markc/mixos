// Fixture: answers x.shared.verb, as does services/beta. Not compiled.

pub fn answer(verb: &str) -> u32 {
    match verb {
        "x.shared.verb" => 1,
        _ => 0,
    }
}
