// Fixture: families are registered but libs/toolkit/src/drive.rs is absent. Not compiled.

pub fn answer(verb: &str) -> u32 {
    match verb {
        "alpha.ping" => 1,
        _ => 0,
    }
}
