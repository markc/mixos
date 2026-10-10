// Fixture (round 27): dispatch::Alpha::ALPHA_NAME is a qualified path to a file-level Type with one
// impl-level "alpha.ping" constant. A qualified Type::NAME is not read, so it is unreadable.
// Not compiled.
pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";
}

pub fn run(verb: &str) -> u8 {
    match verb {
        dispatch::Alpha::ALPHA_NAME => 1,
        "alpha.ping" => 2,
        _ => 0,
    }
}
