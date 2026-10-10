// Fixture (round 15): a br raw byte string with two spaces after its embedded quote. The literal is
// br#"x"  y"#; the exemption naming one space must not match. Not compiled.
fn pick(command: &str, select: fn(&str, bool) -> &str) -> bool {
    if select(br#"x"  y"#, false) == command {
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
