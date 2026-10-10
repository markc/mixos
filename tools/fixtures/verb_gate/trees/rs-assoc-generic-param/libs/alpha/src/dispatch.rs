// Fixture (round 28): Alpha is declared once, at file level, with its own ALPHA_NAME = "alpha.ping".
// A generic parameter named Alpha (wrap<Alpha: Clone>) is another binding of that name in the file,
// so Alpha::ALPHA_NAME is not read and the arm is unreadable (fail closed). Not compiled.
pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";
}

pub fn wrap<Alpha: Clone>(value: Alpha) -> Alpha {
    value
}

pub fn run(verb: &str) -> u8 {
    match verb {
        Alpha::ALPHA_NAME => 1,
        "alpha.ping" => 2,
        _ => 0,
    }
}
