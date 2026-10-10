// Fixture (round 24, decision 2): a pub glob re-export beside the module's own ALPHA_PING. Not compiled.
pub use ::bus::native_session::*;

pub const ALPHA_PING: &str = "alpha.ping";
