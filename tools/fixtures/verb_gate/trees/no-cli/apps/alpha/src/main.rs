// Fixture: the cli root is absent. Not compiled.

pub fn answer(verb: &str) -> u32 {
    match verb {
        "alpha.ping" => 1,
        _ => 0,
    }
}
