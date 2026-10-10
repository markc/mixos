// A client that builds outgoing requests: its guards, its match and its tuple name alpha.ping,
// which alpha answers. Every site is listed in exemptions/builders-listed.conf.mix.
fn build(verb: &str, body: &str) -> (&'static str, &str, u8) {
    let ready = if verb == "alpha.ping" {
        true
    } else {
        false
    };
    let request = ("alpha.ping", body);
    let code = match verb {
        "alpha.ping" => 1,
        _ => 0,
    };
    (request.0, body, code)
}
