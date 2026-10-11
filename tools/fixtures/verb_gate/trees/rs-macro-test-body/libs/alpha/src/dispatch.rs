// Fixture (round 29, ruling (b)): an untrusted macro, and a use of a crate's glob, inside a test
// body only. Test bodies are not compiled into the Bus dispatch, so they cannot change what the
// production arm resolves to: Alpha::ALPHA_NAME reads "alpha.ping" and the tree is clean. Not
// compiled.
pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        match verb {
            Alpha::ALPHA_NAME => 1,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn untrusted_macro_in_a_test_body_is_not_read() {
        some_crate::thing!("x");
        let _ = println_like();
    }
}
