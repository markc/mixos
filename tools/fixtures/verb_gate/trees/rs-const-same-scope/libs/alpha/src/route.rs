// Fixture (round 23): an ordinary literal answer, so the tree has a registration to anchor the scan.
// The registered verb is alpha.ping. Not compiled.

pub fn route(verb: &str) -> u8 {
    match verb {
        "alpha.ping" => 3,
        _ => 0,
    }
}
