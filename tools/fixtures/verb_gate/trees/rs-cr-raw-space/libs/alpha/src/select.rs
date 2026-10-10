// Fixture (round 15): a cr raw C-string with two spaces after its embedded quote. The literal is
// cr#"x"  y"#; the exemption naming one space must not match. Not compiled.
fn pick(command: &str, select: fn(&str, bool) -> &str) -> bool {
    if select(cr#"x"  y"#, false) == command {
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
