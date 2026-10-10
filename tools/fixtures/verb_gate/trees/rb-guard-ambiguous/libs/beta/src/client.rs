// A client with two identical guards for alpha.ping in one fn. One entry for fn check binds both
// sites, so it is ambiguous and fails closed.
fn check(verb: &str) -> bool {
    if verb == "alpha.ping" {
        return true;
    }
    if verb == "alpha.ping" {
        return false;
    }
    false
}
