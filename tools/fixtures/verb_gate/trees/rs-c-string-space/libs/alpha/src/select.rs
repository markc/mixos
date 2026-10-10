// Fixture (round 15): a C string with two spaces inside. The literal is c"x  y"; the exemption
// naming one space must not match. Not compiled.
fn pick(command: &str, select: fn(&str, bool) -> &str) -> bool {
    if select(c"x  y", false) == command {
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
