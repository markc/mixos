// Fixture: a guard, arms and a prefix arm whose names are constants. Not compiled.

pub fn run(verb: &str) -> u8 {
    if verb == ALPHA_PING {
        return 3;
    }
    match verb {
        "alpha.other" => 2,
        ALPHA_OTHER => 1,
        ALPHA_SPLIT
            => 4,
        p if p.starts_with(PREFIX) => 5,
        _ => 0,
    }
}
