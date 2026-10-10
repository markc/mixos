// Fixture: matches! on a verb, then a constant after its closing paren. The constant is not a
// pattern of the matches!, so it names nothing: clean. Not compiled.

const ALPHA_GONE: &str = "alpha.gone";

pub fn run(command: &Command) -> Option<&'static str> {
    let hit = matches!(command.command.as_str(), "alpha.ping").then_some(ALPHA_GONE);
    match command.command.as_str() {
        "alpha.ping" => Some("ready"),
        "alpha.status" => Some("status"),
        _ => None,
    }
}
