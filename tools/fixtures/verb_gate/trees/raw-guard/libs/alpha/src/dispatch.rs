// Fixture: alpha.ping is declared, but no code answers it. Its only mention is in a
// raw string, which holds a guard and a prefix arm naming it. Not compiled.

pub fn describe() -> &'static str {
    r#"
    verb == "alpha.ping"
    x if x.starts_with("alpha.")
    "#
}

pub fn run(verb: &str) -> u8 {
    match verb {
        "alpha.other" => 1,
        _ => 0,
    }
}
