// Fixture (round 25): crate::names::ALPHA_PING is not an item at the top of names.rs (the constant is
// inside inner), so the arm is unreadable (fail closed). The literal arm gives the scan a
// registration to anchor on. Not compiled.
pub fn run(verb: &str) -> u8 {
    match verb {
        crate::names::ALPHA_PING => 1,
        "alpha.ping" => 2,
        _ => 0,
    }
}
