// Fixture (round 26): ALPHA_NAME is an impl-level associated constant of the trait impl, which
// shadows the trait default. Self::ALPHA_NAME reads "alpha.ping", so the arm resolves and the tree
// is clean against registry alpha-ping. Not compiled.
pub trait Named {
    const ALPHA_NAME: &'static str = "alpha.hidden";
}

pub struct Alpha;

impl Named for Alpha {
    const ALPHA_NAME: &'static str = "alpha.ping";

    fn run(&self, verb: &str) -> u8 {
        match verb {
            Self::ALPHA_NAME => 1,
            "alpha.ping" => 2,
            _ => 0,
        }
    }
}
