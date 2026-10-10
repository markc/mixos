// Fixture (round 15): lifetimes and labels are code, not literals. The apostrophes after & and
// before loop labels must not be read as unreadable prefixes. Not compiled.
fn first<'a>(items: &'a [&'a str]) -> &'a str {
    'outer: for item in items {
        break 'outer;
    }
    items[0]
}

pub fn run(command: &Command) -> u8 {
    match command.command.as_str() {
        "alpha.ping" => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
