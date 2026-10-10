// Fixture (round 13): a guard after a constant arm names a literal. The constant is not resolved
// here, so the arm is unreadable, and its message quotes the guard's literal as written, not as
// blanks. The match gives the tree its registrations (the anchor); it is not compiled.
pub fn run(verb: &str) -> u8 {
    match verb {
        "alpha.ping" => 1,
        "alpha.status" => 2,
        ALPHA_GUARD if verb == "alpha.ping" => 3,
        _ => 0,
    }
}
