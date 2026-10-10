// Fixture (round 24, decision 2): names.rs re-exports a glob and defines no ALPHA_PING, so the path
// crate::names::ALPHA_PING reaches something the gate cannot read: unreadable (fail closed). The
// literal arm gives the scan a registration to anchor on. Not compiled.
pub fn run(verb: &str) -> u8 {
    match verb {
        crate::names::ALPHA_PING => 1,
        "alpha.ping" => 2,
        _ => 0,
    }
}
