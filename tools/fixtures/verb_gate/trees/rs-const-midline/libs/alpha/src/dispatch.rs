// Fixture (round 24): a file-level ALPHA_OTHER, and a second one declared mid-line in a fn body after
// a statement. Both are candidates for the bare arm, so it is ambiguous-constant (fail closed), not
// the outer literal. Not compiled.
const ALPHA_OTHER: &str = "alpha.ping";

fn setup() -> u8 {
    let _d = 0; const ALPHA_OTHER: &str = "alpha.other";
    _d
}

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_OTHER => 2,
        _ => 0,
    }
}
