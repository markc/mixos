// Fixture: libs/locked is made unreadable by the runner. Not compiled.

pub fn answer(verb: &str) -> u32 {
    match verb {
        "alpha.ping" => 1,
        _ => 0,
    }
}
