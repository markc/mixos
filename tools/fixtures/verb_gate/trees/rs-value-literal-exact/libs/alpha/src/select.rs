// Fixture (round 13): the same verb comparison as rs-value-literal-changed, with the literal the
// exemption names (alpha.ping). Its literal contents are part of the key, so it is clean and
// counted as reviewed. The match gives the tree its registrations (the anchor); it is not compiled.
fn pick(command: &str, select: fn(&str, bool) -> &str) -> bool {
    if select("alpha.ping", false) == command {
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
