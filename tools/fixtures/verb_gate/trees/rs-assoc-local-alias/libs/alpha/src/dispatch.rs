// Fixture (round 28, sol's medium): the file-level Alpha is declared once, with its own
// ALPHA_NAME = "alpha.ping". Inside run, a fn-local `use helper::Beta as Alpha` rebinds the name,
// so Alpha::ALPHA_NAME there is Beta's "alpha.hidden", not the file-level Alpha's const. The name
// Alpha has two bindings in this file, so the arm is unreadable (fail closed). Not compiled.
pub mod helper {
    pub struct Beta;

    impl Beta {
        pub const ALPHA_NAME: &'static str = "alpha.hidden";
    }
}

pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";
}

pub fn run(verb: &str) -> u8 {
    use helper::Beta as Alpha;

    match verb {
        Alpha::ALPHA_NAME => 1,
        "alpha.ping" => 2,
        _ => 0,
    }
}
