// Fixture (round 26): the impl has no impl-level ALPHA_NAME, so Self::ALPHA_NAME would inherit the
// trait default "alpha.hidden". The gate does not resolve traits, so the arm is unreadable (fail
// closed). Not compiled.
pub trait Named {
    const ALPHA_NAME: &'static str = "alpha.hidden";
}

pub struct Alpha;

impl Named for Alpha {
    fn run(&self, verb: &str) -> u8 {
        match verb {
            Self::ALPHA_NAME => 1,
            "alpha.ping" => 2,
            _ => 0,
        }
    }
}
