// Fixture: a literal that is not a valid verb name must be reported, not dropped.

pub fn answer(verb: &str) -> u32 {
    match verb {
        "Bad.Name" => 1,
        "x.good.one" => 2,
        _ => 0,
    }
}
