// Fixture: a listed test double that is the only code answering alpha.sole, a verb registered to
// alpha. Its answer is not an owner, so alpha.sole is not in code for alpha. Not compiled.

pub fn route(verb: &str) -> u32 {
    match verb {
        "alpha.sole" => 1,
        _ => 0,
    }
}
