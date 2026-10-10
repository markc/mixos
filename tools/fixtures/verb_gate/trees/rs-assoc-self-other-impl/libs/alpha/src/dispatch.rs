// Fixture (round 27): the impl-level ALPHA_NAME = "alpha.ping" sits in an inherent impl of Alpha,
// not in the trait impl whose body holds the arm. Self::ALPHA_NAME reads only the enclosing impl
// block, which does not define it (the trait default is not read), so it is unreadable. Not compiled.
pub trait Named {
    const ALPHA_NAME: &'static str = "alpha.hidden";
}

pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";
}

impl Named for Alpha {
    fn run(&self, verb: &str) -> u8 {
        match verb {
            Self::ALPHA_NAME => 1,
            "alpha.ping" => 2,
            _ => 0,
        }
    }
}
