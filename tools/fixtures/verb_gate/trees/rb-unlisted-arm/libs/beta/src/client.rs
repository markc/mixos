// A keyed match that lists alpha.ping only. Its alpha.build arm is not listed, so it still counts
// as dispatch from beta (fail closed).
fn route(verb: &str) -> u8 {
    match verb {
        "alpha.ping" => 1,
        "alpha.build" => 2,
        _ => 0,
    }
}
