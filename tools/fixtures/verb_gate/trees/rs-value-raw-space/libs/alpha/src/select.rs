// Fixture (round 14): a raw string with two spaces inside. A raw string's contents are literal too:
// the exemption naming one space must not match. The match gives the tree its registrations (the
// anchor); it is not compiled.
fn pick(command: &str, select: fn(&str, bool) -> &str) -> bool {
    if select(r#"a  b"#, false) == command {
        return true;
    }
    false
}

pub fn run(command: &Command) -> u8 {
    match command.command.as_str() {
        "alpha.ping" => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
