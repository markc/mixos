// Fixture: verb comparisons with the verb on the right: a literal and a constant on the left.
// Each names an unregistered verb. Not compiled.
use crate::names::ALPHA_C_GONE;

pub fn run(command: &Command) -> u8 {
    if "alpha.r_gone" == command.command {
        return 6;
    }
    if ALPHA_C_GONE == command.command {
        return 7;
    }
    match command.command.as_str() {
        "alpha.ping" => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
