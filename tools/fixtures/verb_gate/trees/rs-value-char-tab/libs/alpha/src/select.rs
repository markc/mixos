// Fixture (round 14): a char literal holding a TAB where the exemption names a space char. The char
// literal's contents are literal too, so the call is unreviewed and the exemption stale. The match
// gives the tree its registrations (the anchor); it is not compiled.
fn pick(command: &str, select: fn(char, bool) -> &str) -> bool {
    if select('	', false) == command {
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
