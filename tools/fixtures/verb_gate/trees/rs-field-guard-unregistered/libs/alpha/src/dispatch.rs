// Fixture: a negated field-receiver guard whose const resolves to an unregistered verb. Not compiled.
use crate::names::ALPHA_GONE;

const ALPHA_PING: &str = "alpha.ping";

pub fn run(command: &Command) -> u8 {
    if command.command != ALPHA_GONE {
        return 9;
    }
    match command.command.as_str() {
        ALPHA_PING => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
