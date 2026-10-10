// Fixture (round 23): two impls each declare an associated const ALPHA_NAME, and each impl's match
// names its own through Self::. Each site resolves to the definition in its own impl, so the two
// same-name constants are not ambiguous. Both are registered, so clean. Not compiled.

pub struct First;
pub struct Second;

impl First {
    const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        match verb {
            Self::ALPHA_NAME => 1,
            _ => 0,
        }
    }
}

impl Second {
    const ALPHA_NAME: &'static str = "alpha.other";

    pub fn run(&self, verb: &str) -> u8 {
        match verb {
            Self::ALPHA_NAME => 2,
            _ => 0,
        }
    }
}
