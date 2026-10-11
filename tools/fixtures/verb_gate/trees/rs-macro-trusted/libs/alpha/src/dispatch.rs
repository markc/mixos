// Fixture (round 29): trusted macros in statement position. println!, info! and assert! are on
// the reviewed list, so the file's constants still resolve and Alpha::ALPHA_NAME reads
// "alpha.ping": the tree is clean against registry alpha-ping. Not compiled.
pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        println!("run");
        info!("run");
        assert!(verb.len() > 0);
        match verb {
            Alpha::ALPHA_NAME => 1,
            _ => 0,
        }
    }
}
