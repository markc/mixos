// Fixture: a comparison of two runtime values (a cached command and a request's command) is a
// value comparison, not dispatch: clean, and counted in the summary line. Not compiled.

pub fn run(command: &Command, cached: &Cached, request: &Request) -> u8 {
    if cached.command == request.command {
        return 3;
    }
    match command.command.as_str() {
        "alpha.ping" => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
