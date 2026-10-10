// Fixture: matches! on a verb, then a literal after its closing paren. The literal is not a
// pattern of the matches!, so it names nothing: clean. Not compiled.

pub fn run(command: &Command) -> Option<&'static str> {
    let ready = matches!(command.command.as_str(), "alpha.ping").then_some("ready");
    match command.command.as_str() {
        "alpha.ping" => Some("ready"),
        "alpha.status" => Some("status"),
        _ => None,
    }
}
