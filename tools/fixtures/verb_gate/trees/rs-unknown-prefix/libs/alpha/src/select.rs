// Fixture (round 15): zz is not a literal prefix, so zz"x" is a literal the gate cannot read.
// It must fail closed as unreadable literal. Not compiled.
fn pick(command: &str, select: fn(&str, bool) -> &str) -> bool {
    if select(zz"x", false) == command {
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
