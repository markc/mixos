// Fixture (round 27): the impl of Alpha that holds ALPHA_NAME = "alpha.ping" sits inside an inline
// module, so it is not at file level. Alpha::ALPHA_NAME is unreadable, since every impl of Alpha
// in the file must be at file level for a Type:: path to read. Not compiled.
pub struct Alpha;

pub mod inner {
    impl super::Alpha {
        pub const ALPHA_NAME: &'static str = "alpha.ping";
    }
}

impl Alpha {
    pub fn run(&self, verb: &str) -> u8 {
        match verb {
            Alpha::ALPHA_NAME => 1,
            "alpha.ping" => 2,
            _ => 0,
        }
    }
}
