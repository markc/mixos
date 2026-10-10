// Fixture: a lowercase local bound from a literal, compared with a verb expression. The gate
// reads the local as a runtime operand (a known limit): a value comparison, counted, not a
// guard. Not compiled.

pub fn run(command: &Command) -> u8 {
    let v = "alpha.x";
    if command.command == v {
        return 4;
    }
    match command.command.as_str() {
        "alpha.ping" => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
