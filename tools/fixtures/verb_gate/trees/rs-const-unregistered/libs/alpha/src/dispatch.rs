// Fixture: a constant arm that resolves to a verb nothing registers (alpha.gone), beside
// constant arms that resolve and are registered. Not compiled.
use crate::names::ALPHA_OTHER;
use crate::names::ALPHA_SPLIT;

const ALPHA_PING: &str = "alpha.ping";
const ALPHA_GONE: &str = "alpha.gone";

pub fn run(verb: &str) -> u8 {
    match verb {
        ALPHA_PING => 1,
        ALPHA_OTHER => 2,
        ALPHA_SPLIT => 4,
        ALPHA_GONE => 3,
        _ => 0,
    }
}
