// Fixture: a value comparison whose right operand has an opening bracket that never closes. The
// operand cannot be read whole, so the gate must report it unreadable, not pass it silently. The
// match gives the tree its registrations (the anchor); it is not compiled.
pub fn run(verb: &str, cmd: &str, request: &Request) -> u8 {
    if verb == cmd_for(request {
        return 3;
    }
    match verb {
        "alpha.ping" => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
