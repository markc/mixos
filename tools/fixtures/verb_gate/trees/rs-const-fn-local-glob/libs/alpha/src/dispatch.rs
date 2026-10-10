// Fixture (round 25): crate::names::ALPHA_PING reads names.rs's top level, and the fn-local const
// is not an item of names, so the arm is unreadable (fail closed). The literal arm gives the scan a
// registration to anchor on. Not compiled.
pub fn run(verb: &str) -> u8 {
    match verb {
        crate::names::ALPHA_PING => 1,
        "alpha.ping" => 2,
        _ => 0,
    }
}
