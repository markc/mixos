// Fixture: the native.rs echo check as written on main. Its whole operand, `Some(command)`, is
// the reviewed text, so the comparison is clean and counted as reviewed. The match gives the
// tree its registrations (the anchor); it is not compiled.
fn echo_ok(response: &Reply, command: &str) -> bool {
    if response.command_name() != Some(command) {
        return false;
    }
    true
}

pub fn run(command: &Command) -> u8 {
    match command.command.as_str() {
        "alpha.ping" => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
