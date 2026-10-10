// Fixture (round 27): helper::Alpha has its own impl-level ALPHA_NAME = "alpha.ping". The top-level
// Alpha is another type and inherits the trait default "alpha.hidden". Self::ALPHA_NAME in its impl
// has no definition in its own impl block, so it is unreadable and never reads helper's constant.
// Not compiled.
pub trait Named {
    const ALPHA_NAME: &'static str = "alpha.hidden";
}

pub mod helper {
    pub struct Alpha;

    impl Alpha {
        pub const ALPHA_NAME: &'static str = "alpha.ping";
    }
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
