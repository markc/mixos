// Fixture: a negated field-receiver guard whose operand is a const, resolved to its literal. Not compiled.
use crate::names::ALPHA_STATUS;

const ALPHA_PING: &str = "alpha.ping";

pub fn run(command: &Command) -> u8 {
    if command.command != ALPHA_STATUS {
        return 9;
    }
    match command.command.as_str() {
        ALPHA_PING => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
