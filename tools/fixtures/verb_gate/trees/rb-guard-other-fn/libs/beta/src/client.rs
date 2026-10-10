// A client whose guard in fn check is a request builder for alpha.ping. The same guard in fn other
// is not in the entry's fn, so it is dispatch, and beta answers alpha.ping there.
fn check(verb: &str) -> bool {
    if verb == "alpha.ping" {
        true
    } else {
        false
    }
}

fn other(verb: &str) -> bool {
    if verb == "alpha.ping" {
        true
    } else {
        false
    }
}
