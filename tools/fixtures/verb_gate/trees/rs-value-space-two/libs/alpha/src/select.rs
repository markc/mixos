// Fixture (round 14): the literal changes from one space to two. The exemption names the one-space
// key; the call now holds two. Both must be reported: unreviewed, and the exemption stale. The
// match gives the tree its registrations (the anchor); it is not compiled.
fn pick(command: &str, select: fn(&str, bool) -> &str) -> bool {
    if select("  ", false) == command {
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
