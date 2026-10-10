// Fixture (round 26, sol's case): the trait default is "alpha.hidden", and the helper method holds a
// fn-local const ALPHA_NAME = "alpha.ping". Self::ALPHA_NAME is the type's associated constant, which
// Rust reads as the trait default, never the fn local, so the arm is unreadable (fail closed).
// Not compiled.
pub trait Named {
    const ALPHA_NAME: &'static str = "alpha.hidden";
}

pub struct Alpha;

impl Named for Alpha {
    fn helper(&self) -> &'static str {
        const ALPHA_NAME: &str = "alpha.ping";
        ALPHA_NAME
    }

    fn run(&self, verb: &str) -> u8 {
        match verb {
            Self::ALPHA_NAME => 1,
            "alpha.ping" => 2,
            _ => 0,
        }
    }
}
