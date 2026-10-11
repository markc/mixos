// Fixture (round 29): path-qualified trusted macros. tracing::info! and std::println! carry a
// trusted prefix and a reviewed name, so the constants still resolve and Alpha::ALPHA_NAME reads
// "alpha.ping": the tree is clean against registry alpha-ping. Not compiled.
pub struct Alpha;

impl Alpha {
    pub const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        tracing::info!("run");
        std::println!("run");
        match verb {
            Alpha::ALPHA_NAME => 1,
            _ => 0,
        }
    }
}
