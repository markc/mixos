// Fixture (round 29): a known limit, pinned. An untrusted macro in EXPRESSION position (`let v =
// some_crate::thing!(verb);`) is not read by the macro rule, so the file's constants resolve and
// Alpha::ALPHA_NAME reads "alpha.ping": the tree is clean. A macro that expands to an item inside
// an expression is not seen; tools/README.md lists it under known limits. Not compiled.
pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        let _v = some_crate::thing!(verb);
        match verb {
            Alpha::ALPHA_NAME => 1,
            _ => 0,
        }
    }
}
