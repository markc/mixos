// Fixture: five missed dispatch forms, none registered. Not compiled.
// Each must be read and reported as unregistered (exit 1).

pub fn answer(verb: &str, command: &str, n: u32) -> u32 {
    let offset = Some(match verb {
        "x.form.offset" => 1,
        _ => 0,
    });
    let leading = match verb {
        | "x.form.leading" => 2,
        _ => 0,
    };
    let tuple = match (n, command) {
        (0, "x.form.tuple") => 3,
        _ => 0,
    };
    let guard = if verb == "x.form.guard" { 4 } else { 0 };
    let prefix = match command {
        c if c.starts_with("x.form.prefix.") => 5,
        _ => 0,
    };
    offset.unwrap_or(0) + leading + tuple + guard + prefix
}
