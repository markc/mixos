// A client that builds outgoing alpha.ping requests in fn build, and a separate fn answer that
// matches the same head. The entry binds only fn build, so the head in answer is dispatch.
fn build(verb: &str, body: &str) -> String {
    let body = match verb {
        "alpha.ping" => body,
        _ => "",
    };
    body.to_string()
}

fn answer(verb: &str) -> u8 {
    match verb {
        "alpha.ping" => 1,
        _ => 0,
    }
}
