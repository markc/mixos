// Fixture (round 27): Alpha is declared once, at file level, and its one impl is at file level with
// the one impl-level ALPHA_NAME. Alpha::ALPHA_NAME reads "alpha.ping", so the arm resolves and the
// tree is clean against registry alpha-ping. Not compiled.
pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        match verb {
            Alpha::ALPHA_NAME => 1,
            _ => 0,
        }
    }
}
