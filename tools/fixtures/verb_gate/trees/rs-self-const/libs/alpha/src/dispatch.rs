// Fixture: an associated constant of the impl in this file, named as Self::NAME, resolves to
// its literal. Registered, so clean. Not compiled.

const ALPHA_PING: &str = "alpha.ping";

pub struct Gate;

impl Gate {
    const ALPHA_SELF: &'static str = "alpha.status";

    pub fn run(&self, command: &Command) -> u8 {
        if command.command != Self::ALPHA_SELF {
            return 9;
        }
        match command.command.as_str() {
            ALPHA_PING => 1,
            "alpha.status" => 2,
            _ => 0,
        }
    }
}
