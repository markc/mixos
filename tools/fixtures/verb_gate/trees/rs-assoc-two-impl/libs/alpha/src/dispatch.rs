// Fixture (round 26): Alpha has two impl-level ALPHA_NAME constants, an inherent impl and a trait
// impl. Self::ALPHA_NAME and Alpha::ALPHA_NAME are ambiguous, so they are not read as either one.
// Not compiled.
pub trait Named {
    const ALPHA_NAME: &'static str;
}

pub struct Alpha;

impl Alpha {
    const ALPHA_NAME: &'static str = "alpha.ping";
}

impl Named for Alpha {
    const ALPHA_NAME: &'static str = "alpha.hidden";

    fn run(&self, verb: &str) -> u8 {
        match verb {
            Self::ALPHA_NAME => 1,
            "alpha.ping" => 2,
            _ => 0,
        }
    }
}
