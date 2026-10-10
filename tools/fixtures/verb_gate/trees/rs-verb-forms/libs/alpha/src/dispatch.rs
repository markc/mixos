// Fixture: literal verb forms outside a plain == guard: matches!, eq() and a != guard.
// Each names an unregistered verb. Not compiled.

pub fn run(command: &Command) -> u8 {
    if matches!(command.command, "alpha.m_gone") {
        return 3;
    }
    if command.command.eq("alpha.e_gone") {
        return 4;
    }
    if command.command != "alpha.n_gone" {
        return 5;
    }
    match command.command.as_str() {
        "alpha.ping" => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
