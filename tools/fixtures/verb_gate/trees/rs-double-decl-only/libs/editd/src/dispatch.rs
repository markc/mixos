// Fixture: editd answers its own verb, edit.ping. Not compiled.

pub fn route(verb: &str) -> u32 {
    match verb {
        "edit.ping" => 1,
        _ => 0,
    }
}
