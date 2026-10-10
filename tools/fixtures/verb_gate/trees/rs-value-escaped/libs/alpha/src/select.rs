// Fixture (round 14): an escaped quote inside the literal, then two spaces. The escaped quote does
// not end the literal, so the two spaces are literal contents and the exact key keeps them; the
// one-space exemption must not match. The match gives the tree its registrations (the anchor); it
// is not compiled.
fn pick(command: &str, select: fn(&str, bool) -> &str) -> bool {
    if select("x\"  y", false) == command {
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
