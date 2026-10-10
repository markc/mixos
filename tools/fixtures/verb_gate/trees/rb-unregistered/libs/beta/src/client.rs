// A client whose guard names beta.nope, a verb the registry does not hold.
fn check(verb: &str) -> bool {
    if verb == "beta.nope" {
        true
    } else {
        false
    }
}
