// Fixture (round 13): a verb comparison whose call holds a literal argument. The exemption names
// select("alpha.ping", false) == command; this file changes the literal to an equal-length one
// (alpha.gone), so the comparison is unreviewed and the exemption is stale. The match gives the
// tree its registrations (the anchor); it is not compiled.
fn pick(command: &str, select: fn(&str, bool) -> &str) -> bool {
    if select("alpha.gone", false) == command {
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
