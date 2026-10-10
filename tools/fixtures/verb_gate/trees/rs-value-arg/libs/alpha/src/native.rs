// Fixture: the native.rs echo check with its argument changed from `command` to
// `request.command`. The exemption names `Some(command)`, so this comparison is unreviewed and the
// exemption is stale. The match gives the tree its registrations (the anchor); it is not compiled.
fn echo_ok(response: &Reply, request: &Request) -> bool {
    if response.command_name() != Some(request.command) {
        return false;
    }
    true
}

pub fn run(command: &Command) -> u8 {
    match command.command.as_str() {
        "alpha.ping" => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
