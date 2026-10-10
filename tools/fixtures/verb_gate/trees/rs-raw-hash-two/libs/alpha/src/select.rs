// Fixture (round 15): a raw string with a two-hash delimiter whose content holds "# and two spaces.
// The literal is r##"a"#  b"##: the "# inside is content, not the close. The exemption naming one
// space must not match. Not compiled.
fn pick(command: &str, select: fn(&str, bool) -> &str) -> bool {
    if select(r##"a"#  b"##, false) == command {
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
