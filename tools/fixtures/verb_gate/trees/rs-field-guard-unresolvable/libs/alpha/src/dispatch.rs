// Fixture: field-receiver guards whose operands the gate cannot resolve: a const named nowhere,
// an eq() argument, and a Self:: constant named nowhere. Registered arms keep the scan anchored. Not compiled.

const ALPHA_PING: &str = "alpha.ping";

pub fn run(command: &Command) -> u8 {
    if command.command != ALPHA_MISSING {
        return 9;
    }
    if command.command.eq(ALPHA_NOPE) {
        return 8;
    }
    if command.command == Self::ALPHA_NOWHERE {
        return 7;
    }
    match command.command.as_str() {
        ALPHA_PING => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
