// Fixture (round 25, the variant without the glob): crate::names::ALPHA_PING is a fn-local const in
// names.rs, not a module item, so the arm is unreadable (fail closed). The literal arm gives the scan
// a registration to anchor on. Not compiled.
pub fn run(verb: &str) -> u8 {
    match verb {
        crate::names::ALPHA_PING => 1,
        "alpha.ping" => 2,
        _ => 0,
    }
}
