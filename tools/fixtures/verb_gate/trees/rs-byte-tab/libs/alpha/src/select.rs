// Fixture (round 15): a byte char holding a tab escape. The literal is b'\t' (four characters);
// the exemption naming b' ' (a space) must not match. Not compiled.
fn pick(command: &str, select: fn(&str, bool) -> &str) -> bool {
    if select(b'\t', false) == command {
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
