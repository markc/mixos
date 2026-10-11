// Fixture (round 29): a local macro_rules! named println. Its own definition can shadow the std
// macro the file invokes, so the file's constants are not read: Alpha::ALPHA_NAME is unreadable
// (fail closed). Not compiled.
macro_rules! println {
    ($($t:tt)*) => {};
}

pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        println!("run");
        match verb {
            Alpha::ALPHA_NAME => 1,
            "alpha.ping" => 2,
            _ => 0,
        }
    }
}
