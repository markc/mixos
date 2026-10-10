// Fixture (round 24): two impls each declare ALPHA_NAME, and each match names it bare. A bare name is
// counted over the whole file, so two candidates are ambiguous-constant (fail closed). The Self::
// form (rs-const-two-impls) stays clean. Not compiled.
pub struct First;
pub struct Second;

impl First {
    const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        match verb {
            ALPHA_NAME => 1,
            _ => 0,
        }
    }
}

impl Second {
    const ALPHA_NAME: &'static str = "alpha.other";

    pub fn run(&self, verb: &str) -> u8 {
        match verb {
            ALPHA_NAME => 2,
            _ => 0,
        }
    }
}
