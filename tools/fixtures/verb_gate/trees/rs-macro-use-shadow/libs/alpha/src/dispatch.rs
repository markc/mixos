// Fixture (round 29): #[macro_use] brings a crate's macros into scope by name, so any of them can
// shadow a macro the file invokes. The file's constants are not read: Alpha::ALPHA_NAME is
// unreadable (fail closed). Not compiled.
#[macro_use]
extern crate some_crate;

pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        match verb {
            Alpha::ALPHA_NAME => 1,
            "alpha.ping" => 2,
            _ => 0,
        }
    }
}
