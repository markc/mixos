// Fixture (round 29): sol's macro alias. `println` is rebound by a use of a crate's item, so the
// println! below is an import the gate cannot read. Alpha::ALPHA_NAME is unreadable (fail closed).
// Not compiled.
use some_crate::thing as println;

pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        println!("run");
        match verb {
            Alpha::ALPHA_NAME => 1,
            "alpha.ping" => 2,
            _ => 0,
        }
    }
}
