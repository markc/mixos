// Fixture (round 25): names.rs holds an inline module whose top level defines ALPHA_PING, so
// crate::names::inner::ALPHA_PING reads that item (clean). Not compiled.
pub mod inner {
    pub const ALPHA_PING: &str = "alpha.ping";
}
