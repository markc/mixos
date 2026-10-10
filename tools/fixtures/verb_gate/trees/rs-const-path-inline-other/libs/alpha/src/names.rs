// Fixture (round 25): ALPHA_PING lives in the inline module inner, not at the top of names.rs, so
// crate::names::ALPHA_PING does not reach it: unreadable (fail closed). Not compiled.
pub mod inner {
    pub const ALPHA_PING: &str = "alpha.ping";
}
