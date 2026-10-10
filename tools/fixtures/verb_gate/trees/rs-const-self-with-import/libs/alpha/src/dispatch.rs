// Fixture (round 24): Self::ALPHA_NAME names the associated const of its own impl, even though the
// file also imports a different ALPHA_NAME. The path says Self, so it resolves to alpha.ping and is
// clean. Not compiled.
use crate::names::ALPHA_NAME;

pub struct First;

impl First {
    const ALPHA_NAME: &'static str = "alpha.ping";

    pub fn run(&self, verb: &str) -> u8 {
        match verb {
            Self::ALPHA_NAME => 1,
            _ => 0,
        }
    }
}
