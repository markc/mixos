// Fixture (round 29): an untrusted path-qualified macro in statement position. some_crate::thing!
// is not on the reviewed list, so the file's constants are not read: Alpha::ALPHA_NAME is
// unreadable (fail closed). Not compiled.
pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        some_crate::thing!(verb);
        match verb {
            Alpha::ALPHA_NAME => 1,
            "alpha.ping" => 2,
            _ => 0,
        }
    }
}
