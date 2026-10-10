// Fixture (round 23): two file-level consts with the same name ALPHA_DUP, one naming alpha.ping and
// one alpha.other. Neither binds by scope, so the arm that names ALPHA_DUP is ambiguous-constant
// (fail closed) and is not silently read as the last one. Not compiled.

const ALPHA_DUP: &str = "alpha.ping";
const ALPHA_DUP: &str = "alpha.other";

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_DUP => 1,
        _ => 0,
    }
}
